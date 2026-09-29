// This viewer on this network: a read-only app with no sign-in, and the rules it can verify.
// The folder walk, the verifier and the polling are the browser client's (`ChainReader`).

import { MayflyApp, RulesRegistry, shippedRules, type Store } from "@synonymdev/mayfly-browser";
import wasmUrl from "@synonymdev/mayfly/pkg/mayfly_bg.wasm?url";

import { contractRules } from "../../contract/src/rules";

export const IS_TESTNET = import.meta.env.VITE_TESTNET === "true";

/** The client id is only a folder name here; a viewer writes nothing. */
export const app = new MayflyApp({ clientId: "view.mayfly.example", testnet: IS_TESTNET, wasm: wasmUrl });

let ready: Promise<{ store: Store; registry: RulesRegistry }> | undefined;

/**
 * The read-only store and the rules this build can run, once. Shipped rules (`list/1`,
 * `chess/1`) plus `contract/1`. A chain whose rules are not here is still listed and
 * decoded; the page shows those records without a verified view.
 */
export function reader(): Promise<{ store: Store; registry: RulesRegistry }> {
  ready ??= app.readOnlyStore().then((store) => ({
    store,
    registry: new RulesRegistry(shippedRules(), [contractRules]),
  }));
  ready.catch(() => {
    ready = undefined;
  });
  return ready;
}
