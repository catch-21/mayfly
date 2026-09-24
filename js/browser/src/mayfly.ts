// The wasm module, loaded once, and the thin typed layer over its exports.

import init, {
  ChainClient,
  decodeRecord as decodeRecordWasm,
  parseChainUrl as parseChainUrlWasm,
  rulesIds,
  verifyFrom as verifyFromWasm,
  type InitInput,
} from "@synonymdev/mayfly";
import type { Signer, Store } from "@synonymdev/mayfly/js/pubky-glue.js";

import type { RulesRef } from "./rules.js";
import type { ChainRef, ChainView, RecordView } from "./types.js";

export type { Signer, Store };
export { ChainClient };

let ready: Promise<void> | undefined;

/**
 * Load the wasm module once. Pass where the bytes are when the bundler does not resolve
 * them itself — with Vite, `import wasmUrl from "@synonymdev/mayfly/pkg/mayfly_bg.wasm?url"`.
 * Later calls share the first promise; a failed load is forgotten so a retry can succeed.
 */
export function loadMayfly(wasm?: InitInput | Promise<InitInput>): Promise<void> {
  if (!ready) {
    ready = init(wasm === undefined ? undefined : { module_or_path: wasm }).then(() => undefined);
    ready.catch(() => {
      ready = undefined;
    });
  }
  return ready;
}

/** The rules ids the wasm module ships. Call after `loadMayfly`. */
export function shippedRules(): string[] {
  return rulesIds();
}

const RECORD_IN_CHAIN = /^(pubky:\/\/[^/]+\/.*?\/mayfly\/chains\/[0-9A-Z]{26})\/?/;

/**
 * Parse a chain URL, or a link to any record inside a chain folder, into its parts and the
 * canonical folder URL. Throws with a readable message otherwise.
 */
export function parseChainUrl(input: string): ChainRef {
  const trimmed = input.trim();
  const m = RECORD_IN_CHAIN.exec(trimmed);
  const url = m ? `${m[1]}/` : trimmed;
  const parsed = parseChainUrlWasm(url) as Omit<ChainRef, "url">;
  return { ...parsed, url };
}

/** Whether `input` is a chain or record URL, without throwing. */
export function isChainUrl(input: string): boolean {
  try {
    parseChainUrl(input);
    return true;
  } catch {
    return false;
  }
}

/** Verify a chain from files alone, as anyone (§9). */
export function verifyFrom<S = unknown>(rules: RulesRef, store: Store, chainUrl: string): Promise<ChainView<S>> {
  return verifyFromWasm(rules, store, chainUrl) as Promise<ChainView<S>>;
}

/** Decode one record file for inspection (§14). */
export function decodeRecord(bytes: Uint8Array, name: string, etag?: string): RecordView {
  return decodeRecordWasm(bytes, name, etag ?? null) as RecordView;
}
