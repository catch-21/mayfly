// The seam to the two wasm modules: initialise Mayfly once, build a read-only store over the
// Pubky SDK, and load a chain — detect its rules from genesis, verify it, and walk every folder
// it declares so the files panel can show each record with its checks.

import init, { verifyFrom, decodeRecord, parseChainUrl, rulesIds } from "@synonymdev/mayfly";
import wasmUrl from "@synonymdev/mayfly/pkg/mayfly_bg.wasm?url";
import { storeFromPubky } from "@synonymdev/mayfly/js/pubky-glue.js";
import type { Store } from "@synonymdev/mayfly/js/pubky-glue.js";
import { Pubky } from "@synonymdev/pubky";

import type { ChainView, Folder, LoadedFile, RecordView } from "./types";
import { describeError } from "./format";

export const IS_TESTNET = import.meta.env.VITE_TESTNET === "true";

let ready: Promise<Store> | null = null;

/** Initialise the wasm module and the SDK facade once; later calls share the promise. */
export function store(): Promise<Store> {
  if (!ready) {
    ready = (async () => {
      await init({ module_or_path: wasmUrl });
      const pubky = IS_TESTNET ? Pubky.testnet() : new Pubky();
      return storeFromPubky(pubky);
    })();
    // A failed initialisation (the wasm did not load) must not poison every later call.
    ready.catch(() => {
      ready = null;
    });
  }
  return ready;
}

/** The rules ids this build of the module carries. Safe to call only after `store()`. */
export function knownRules(): string[] {
  return rulesIds();
}

/** A parsed chain URL. */
export interface ChainRef {
  chain: string;
  owner: string;
  /** The initiator's protocol folder, e.g. `/pub/list.example/mayfly/`. */
  folder: string;
  /** The canonical chain URL, `pubky://<owner><folder>chains/<chain>/`. */
  url: string;
}

const RECORD_IN_CHAIN = /^(pubky:\/\/[^/]+\/.*?\/mayfly\/chains\/[0-9A-Z]{26})\/?/;

/**
 * Accept a link to the chain folder or to any record inside it and return the chain folder URL.
 * Throws with a readable message when the link is not a Mayfly chain URL.
 */
export function normaliseChainUrl(input: string): ChainRef {
  const trimmed = input.trim();
  const m = RECORD_IN_CHAIN.exec(trimmed);
  const url = m ? `${m[1]}/` : trimmed;
  const parsed = parseChainUrl(url) as { chain: string; owner: string; folder: string };
  return { ...parsed, url };
}

/** What `loadChain` returns: everything the page renders. */
export interface Loaded {
  ref: ChainRef;
  /** Rules id read from genesis; `null` when genesis could not be found or read. */
  rules: string | null;
  /** Whether this build carries `rules`. */
  rulesKnown: boolean;
  /** The verified chain, when verification ran and succeeded. */
  view: ChainView | null;
  /** Why there is no view: no genesis yet, unknown rules, a store error. */
  viewError: string | null;
  /** Every folder walked, including ones whose listing failed. */
  folders: Folder[];
  /** Every file listed, decoded where possible. */
  files: LoadedFile[];
  loadedAt: Date;
}

/** Decoded records keyed by owner, path and ETag, so a poll only refetches changed files. */
const recordCache = new Map<string, RecordView>();

function cacheKey(owner: string, path: string, etag: string): string {
  return `${owner}${path}@${etag}`;
}

async function fetchRecord(s: Store, file: LoadedFile): Promise<void> {
  const { owner } = file.folder;
  if (file.contentHash) {
    const hit = recordCache.get(cacheKey(owner, file.path, file.contentHash));
    if (hit) {
      file.record = hit;
      return;
    }
  }
  let bytes: Uint8Array | null | undefined;
  try {
    bytes = await s.get(owner, file.path);
  } catch (e) {
    file.fetchError = describeError(e);
    return;
  }
  if (!bytes) {
    file.fetchError = "file not found when fetched (listed, then gone)";
    return;
  }
  const record = decodeRecord(bytes, file.path, file.contentHash) as RecordView;
  file.record = record;
  if (file.contentHash) {
    recordCache.set(cacheKey(owner, file.path, file.contentHash), record);
  }
}

/** Run `f` over `items` at most `width` at a time. */
async function inBatches<T>(items: T[], width: number, f: (t: T) => Promise<void>): Promise<void> {
  for (let i = 0; i < items.length; i += width) {
    await Promise.all(items.slice(i, i + width).map(f));
  }
}

/** List one folder and decode every `.jws` inside it. Listing failure is recorded, not thrown. */
async function walkFolder(s: Store, folder: Folder): Promise<LoadedFile[]> {
  let listed: { path: string; contentHash?: string }[];
  try {
    listed = await s.list(folder.owner, folder.prefix);
  } catch (e) {
    folder.error = describeError(e);
    return [];
  }
  const files: LoadedFile[] = listed.map((l) => ({
    folder,
    path: l.path,
    relative: l.path.startsWith(folder.prefix) ? l.path.slice(folder.prefix.length) : l.path,
    contentHash: l.contentHash,
  }));
  await inBatches(
    files.filter((f) => f.path.endsWith(".jws")),
    8,
    (f) => fetchRecord(s, f),
  );
  return files;
}

/** The genesis file in a chain folder listing, if present. */
function genesisOf(files: LoadedFile[]): LoadedFile | undefined {
  return files.find((f) => /^links\/00000000-[^/]+\.jws$/.test(f.relative));
}

function endsWithSlash(p: string): string {
  return p.endsWith("/") ? p : `${p}/`;
}

/**
 * Load a chain: list the initiator's folder, read the rules id from genesis, verify with the
 * module, then walk every folder the verified head declares (seats and witnesses).
 */
export async function loadChain(input: string): Promise<Loaded> {
  const s = await store();
  const ref = normaliseChainUrl(input);
  const chainDir = `chains/${ref.chain}/`;

  const initiator: Folder = {
    role: "initiator",
    owner: ref.owner,
    prefix: `${ref.folder}${chainDir}`,
  };
  const folders: Folder[] = [initiator];
  const files: LoadedFile[] = await walkFolder(s, initiator);

  // Rules detection: genesis carries the rules id in its body.
  let rules: string | null = null;
  const genesis = genesisOf(files);
  if (genesis?.record) {
    const payload = genesis.record.payload as { body?: { rules?: unknown } } | null;
    const r = payload?.body?.rules;
    if (typeof r === "string") rules = r;
  }
  const known = knownRules();
  const rulesKnown = rules !== null && known.includes(rules);

  let view: ChainView | null = null;
  let viewError: string | null = null;
  if (initiator.error) {
    viewError = `could not list the initiator's folder: ${initiator.error}`;
  } else if (!genesis) {
    viewError = "no genesis yet: the initiator's chain folder has no links/00000000-<h16>.jws";
  } else if (!genesis.record) {
    viewError = `genesis could not be read: ${genesis.fetchError ?? "unknown error"}`;
  } else if (rules === null) {
    viewError = genesis.record.error
      ? `genesis is not a readable record: ${genesis.record.error}`
      : "genesis carries no rules id in its body";
  } else if (!rulesKnown) {
    viewError = `this build does not carry rules ${JSON.stringify(rules)}; it knows ${known.join(", ")}`;
  } else {
    try {
      view = (await verifyFrom(rules, s, ref.url)) as ChainView;
    } catch (e) {
      viewError = describeError(e);
    }
  }

  // Folder walk: every seat's declared paths and every engaged witness's folder, deduplicated
  // against the initiator folder already listed.
  if (view) {
    const seen = new Set<string>([`${initiator.owner}${initiator.prefix}`]);
    const more: Folder[] = [];
    for (const seat of view.seats) {
      for (const p of seat.paths) {
        const prefix = `${endsWithSlash(p)}${chainDir}`;
        const key = `${seat.pubky}${prefix}`;
        if (seen.has(key)) continue;
        seen.add(key);
        more.push({ role: "seat", owner: seat.pubky, prefix });
      }
    }
    for (const w of view.engaged) {
      const prefix = `${endsWithSlash(w.path)}witness/${ref.chain}/`;
      const key = `${w.pubky}${prefix}`;
      if (seen.has(key)) continue;
      seen.add(key);
      more.push({ role: "witness", owner: w.pubky, prefix });
    }
    const walked = await Promise.all(more.map((f) => walkFolder(s, f)));
    folders.push(...more);
    for (const w of walked) files.push(...w);
  }

  return {
    ref,
    rules,
    rulesKnown,
    view,
    viewError,
    folders,
    files,
    loadedAt: new Date(),
  };
}
