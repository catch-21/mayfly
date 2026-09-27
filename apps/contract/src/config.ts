// This app on this network, and the `contract/1` shapes it renders. Everything Pubky and
// Mayfly is the browser client's; what is left here is the contract.

import { MayflyApp, RulesRegistry, shippedRules } from "@synonymdev/mayfly-browser";
import wasmUrl from "@synonymdev/mayfly/pkg/mayfly_bg.wasm?url";

import { contractRules } from "./rules";

/** Testnet flavour: `VITE_TESTNET=true` at build time. */
export const TESTNET = import.meta.env.VITE_TESTNET === "true";

/** The app's client id names the folder every record lives under (`/pub/<clientId>/mayfly/`). */
export const app = new MayflyApp({
  clientId: (import.meta.env.VITE_CLIENT_ID as string | undefined) ?? "contract.mayfly.example",
  testnet: TESTNET,
  wasm: wasmUrl,
});

export { contractRules };

/** Shipped rules plus `contract/1`, so a reader can verify a contract chain. Call after wasm has loaded. */
export function contractRegistry(): RulesRegistry {
  return new RulesRegistry(shippedRules(), [contractRules]);
}
