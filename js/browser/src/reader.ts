// Reading a chain as anyone: list the initiator's folder, read the rules id from genesis,
// verify with the module, then walk every folder the verified head declares so a page can
// show each record with its checks (§9, §14). No seat, no signer, no votes.

import { describeError } from "./errors.js";
import { decodeRecord, parseChainUrl, verifyFrom, type Store } from "./mayfly.js";
import type { RulesRegistry } from "./rules.js";
import type { ChainRef, ChainView, RecordView } from "./types.js";

/** Which of the chain's declared folders a file was read from. */
export type FolderRole = "initiator" | "seat" | "witness";

/** One folder the reader lists. */
export interface Folder {
  role: FolderRole;
  owner: string;
  /** Absolute prefix listed, ending in `/`. */
  prefix: string;
  /** Why the listing failed, if it did. */
  error?: string;
}

/** One listed file, decoded when it is a `.jws`. */
export interface LoadedFile {
  folder: Folder;
  /** Absolute path on the owner's homeserver. */
  path: string;
  /** Path relative to the folder prefix. */
  relative: string;
  /** The homeserver's ETag, when the listing reported one. */
  contentHash?: string;
  /** Decoded record; absent for files that are not `.jws` or that could not be fetched. */
  record?: RecordView;
  /** Why the bytes could not be fetched, if they could not. */
  fetchError?: string;
}

/** What `loadChain` returns: everything a page renders. */
export interface Loaded<S = unknown> {
  ref: ChainRef;
  /** Rules id read from genesis; `null` when genesis could not be found or read. */
  rules: string | null;
  /** Whether the registry can run `rules`. */
  rulesKnown: boolean;
  /** The verified chain, when verification ran and succeeded. */
  view: ChainView<S> | null;
  /** Why there is no view: no genesis yet, unknown rules, a store error. */
  viewError: string | null;
  /** Every folder walked, including ones whose listing failed. */
  folders: Folder[];
  /** Every file listed, decoded where possible. */
  files: LoadedFile[];
  loadedAt: Date;
}

/** A stable key for one listed file. */
export function fileKey(f: LoadedFile): string {
  return `${f.folder.owner}${f.path}`;
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
  const record = decodeRecord(bytes, file.path, file.contentHash);
  file.record = record;
  if (file.contentHash) recordCache.set(cacheKey(owner, file.path, file.contentHash), record);
}

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

function genesisOf(files: LoadedFile[]): LoadedFile | undefined {
  return files.find((f) => /^links\/00000000-[^/]+\.jws$/.test(f.relative));
}

function endsWithSlash(p: string): string {
  return p.endsWith("/") ? p : `${p}/`;
}

/**
 * Load a chain from a chain or record URL. `registry` decides which rules can be run; a chain
 * whose rules it lacks is still listed and decoded, with `viewError` saying why there is no
 * verified view.
 */
export async function loadChain<S = unknown>(store: Store, registry: RulesRegistry, input: string): Promise<Loaded<S>> {
  const ref = parseChainUrl(input);
  const chainDir = `chains/${ref.chain}/`;

  const initiator: Folder = { role: "initiator", owner: ref.owner, prefix: `${ref.folder}${chainDir}` };
  const folders: Folder[] = [initiator];
  const files: LoadedFile[] = await walkFolder(store, initiator);

  let rules: string | null = null;
  const genesis = genesisOf(files);
  if (genesis?.record) {
    const payload = genesis.record.payload as { body?: { rules?: unknown } } | null;
    const r = payload?.body?.rules;
    if (typeof r === "string") rules = r;
  }
  const runnable = rules === null ? undefined : registry.resolve(rules);

  let view: ChainView<S> | null = null;
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
  } else if (runnable === undefined) {
    viewError = `this app does not carry rules ${JSON.stringify(rules)}; it knows ${registry.ids().join(", ")}`;
  } else {
    try {
      view = await verifyFrom<S>(runnable, store, ref.url);
    } catch (e) {
      viewError = describeError(e);
    }
  }

  // Every seat's declared paths and every engaged witness's folder, deduplicated against
  // the initiator folder already listed.
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
    const walked = await Promise.all(more.map((f) => walkFolder(store, f)));
    folders.push(...more);
    for (const w of walked) files.push(...w);
  }

  return { ref, rules, rulesKnown: runnable !== undefined, view, viewError, folders, files, loadedAt: new Date() };
}

export const DEFAULT_POLL_MS = 4_000;

export interface ReaderState<S = unknown> {
  url: string;
  loaded: Loaded<S> | undefined;
  /** Why the chain could not be loaded at all (a bad link). */
  error: string | undefined;
  busy: boolean;
  /** Polling has stopped because the chain is final. */
  final: boolean;
}

export interface ChainReaderOptions {
  store: Store;
  registry: RulesRegistry;
  url: string;
  pollMs?: number;
}

/**
 * Follow a chain as a bystander: load it, poll while it is open or has no view yet, and stop
 * once the verifier says it is final.
 */
export class ChainReader<S = unknown> {
  private listeners = new Set<(s: ReaderState<S>) => void>();
  private timer: ReturnType<typeof setInterval> | undefined;
  private inFlight = false;
  private stopped = false;
  private _state: ReaderState<S>;
  private readonly pollMs: number;

  constructor(private readonly options: ChainReaderOptions) {
    this.pollMs = options.pollMs ?? DEFAULT_POLL_MS;
    this._state = { url: options.url, loaded: undefined, error: undefined, busy: false, final: false };
  }

  get state(): ReaderState<S> {
    return this._state;
  }

  subscribe(listener: (s: ReaderState<S>) => void): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  /** Load now and poll until the chain is final. `refresh()` alone works without this. */
  start(): void {
    if (this.timer) return;
    this.stopped = false;
    void this.refresh();
    this.timer = setInterval(() => {
      if (!this._state.final) void this.refresh();
    }, this.pollMs);
  }

  stop(): void {
    this.stopped = true;
    if (this.timer) clearInterval(this.timer);
    this.timer = undefined;
  }

  async refresh(): Promise<void> {
    if (this.inFlight) return;
    this.inFlight = true;
    this.set({ busy: true });
    try {
      const loaded = await loadChain<S>(this.options.store, this.options.registry, this.options.url);
      if (this.stopped) return;
      this.set({ loaded, error: undefined, final: loaded.view?.is_final ?? false });
    } catch (e) {
      if (!this.stopped) this.set({ error: describeError(e) });
    } finally {
      this.inFlight = false;
      this.set({ busy: false });
    }
  }

  private set(patch: Partial<ReaderState<S>>): void {
    this._state = { ...this._state, ...patch };
    for (const l of this.listeners) l(this._state);
  }
}
