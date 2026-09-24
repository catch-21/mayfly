// Home: the chains a member is on, from the index markers in their own folder (§7); creating
// a chain; and checking a list of pubkys before a genesis is written. Nothing here needs the
// Pubky SDK, so it runs in Node over a memory store.

import { ChainClient, type Signer, type Store } from "./mayfly.js";
import type { RulesRef } from "./rules.js";
import type { ChainSpec } from "./types.js";

/** One chain a member is on. */
export interface MyChain {
  url: string;
  finished: boolean;
}

/**
 * My chains, from the `index/active/` and `index/finished/` markers under `folder`. Finished
 * is read first so that, if an active marker was left behind, the chain is still shown once
 * and as finished.
 */
export async function myChains(store: Store, folder: string): Promise<MyChain[]> {
  const byUrl = new Map<string, boolean>();
  for (const [sub, finished] of [
    ["index/finished/", true],
    ["index/active/", false],
  ] as const) {
    const listed = await store.list(store.me, folder + sub);
    for (const entry of listed) {
      const bytes = await store.get(store.me, entry.path);
      if (!bytes) continue;
      const url = new TextDecoder().decode(bytes).trim();
      if (!byUrl.has(url)) byUrl.set(url, finished);
    }
  }
  return [...byUrl].map(([url, finished]) => ({ url, finished }));
}

/**
 * Create a chain: genesis names every party (the creator first) and, optionally, watchmen.
 * Returns the invite URL. The client is discarded; open the URL with a `ChainSession`.
 */
export async function createChain(rules: RulesRef, store: Store, signer: Signer, spec: ChainSpec): Promise<string> {
  const parties = [signer.pubky, ...spec.parties.filter((p) => p !== signer.pubky)];
  const client = await ChainClient.create(rules, store, signer, {
    ...spec,
    parties,
    apps: spec.apps ?? parties.map(() => signer.clientId),
  });
  return client.inviteUrl();
}

const Z32 = /^[ybndrfg8ejkmcpqxot1uwisza345h769]{52}$/;

/** Whether `s` has the shape of a pubky (z-base-32, 52 characters). */
export function looksLikePubky(s: string): boolean {
  return Z32.test(s);
}

/**
 * Pubkys from free text — one per line, or separated by commas or spaces. Blanks and
 * `exclude` (the person typing) are dropped. A repeat, or anything that is not a pubky,
 * throws with a message naming it: genesis would refuse the same thing later and less
 * helpfully.
 */
export function parsePubkys(raw: string, options: { exclude?: string; isPubky?: (s: string) => boolean } = {}): string[] {
  const isPubky = options.isPubky ?? looksLikePubky;
  const seen = new Set<string>();
  const out: string[] = [];
  for (const piece of raw.split(/[\s,]+/)) {
    const p = piece.trim();
    if (!p || p === options.exclude) continue;
    if (seen.has(p)) throw new Error(`"${p}" is listed more than once`);
    if (!isPubky(p)) throw new Error(`"${p}" is not a pubky`);
    seen.add(p);
    out.push(p);
  }
  return out;
}
