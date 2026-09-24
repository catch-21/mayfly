// The wasm module, loaded once, and the thin typed layer over its exports.

import init, {
  ChainClient,
  ChainReader as WasmReader,
  decodeRecord as decodeRecordWasm,
  isPubky as isPubkyWasm,
  myChains as myChainsWasm,
  parseChainUrl as parseChainUrlWasm,
  rulesIds,
  verifyFrom as verifyFromWasm,
  type InitInput,
} from "@synonymdev/mayfly";
import type { Signer, Store } from "@synonymdev/mayfly/js/pubky-glue.js";

import type { RulesRef } from "./rules.js";
import type { ChainRef, ChainView, Loaded, MyChain, RecordView } from "./types.js";

export type { Signer, Store };
export { ChainClient, WasmReader };

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

/**
 * Parse a chain URL, or a link to any record inside a chain folder, into its parts and the
 * canonical folder URL. Throws with a readable message otherwise.
 */
export function parseChainUrl(input: string): ChainRef {
  return parseChainUrlWasm(input) as ChainRef;
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

/** Whether `s` is a pubky: z-base-32 that decodes to a public key. */
export function isPubky(s: string): boolean {
  return isPubkyWasm(s);
}

/** Verify a chain from files alone, as anyone (§9). */
export function verifyFrom<S = unknown>(rules: RulesRef, store: Store, chainUrl: string): Promise<ChainView<S>> {
  return verifyFromWasm(rules, store, chainUrl) as Promise<ChainView<S>>;
}

/** Decode one record file for inspection (§14). */
export function decodeRecord(bytes: Uint8Array, name: string, etag?: string): RecordView {
  return decodeRecordWasm(bytes, name, etag ?? null) as RecordView;
}

/**
 * My chains, from the `index/active/` and `index/finished/` markers under `folder` (§7).
 * Finished first; a chain with both markers is listed once, as finished.
 */
export function myChains(store: Store, folder: string): Promise<MyChain[]> {
  return myChainsWasm(store, folder) as Promise<MyChain[]>;
}

/** One load of a chain through the module's reader. */
export function loadWith<S = unknown>(reader: WasmReader, url: string): Promise<Loaded<S>> {
  return reader.load(url) as Promise<Loaded<S>>;
}
