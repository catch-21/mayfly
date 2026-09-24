// Creating a chain, and checking a list of pubkys before a genesis is written.

import { ChainClient, isPubky, type Signer, type Store } from "./mayfly.js";
import type { RulesRef } from "./rules.js";
import type { ChainSpec } from "./types.js";

export { myChains } from "./mayfly.js";

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

/**
 * Pubkys from free text — one per line, or separated by commas or spaces. Blanks and
 * `exclude` (the person typing) are dropped. A repeat, or anything that is not a pubky,
 * throws with a message naming it: genesis would refuse the same thing later and less
 * helpfully.
 */
export function parsePubkys(raw: string, options: { exclude?: string } = {}): string[] {
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
