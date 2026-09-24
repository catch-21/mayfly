// This app on this network, and the `list/1` shapes it renders. Everything Pubky and Mayfly
// is the browser client's; what is left here is what makes this the shopping list.

import { MayflyApp } from "@synonymdev/mayfly-browser";
import wasmUrl from "@synonymdev/mayfly/pkg/mayfly_bg.wasm?url";

/** Testnet flavour: `VITE_TESTNET=true` at build time. */
export const TESTNET = import.meta.env.VITE_TESTNET === "true";

/** The viewer, for "open in viewer" links; `undefined` hides them. */
export const VIEWER_URL = import.meta.env.VITE_VIEWER_URL as string | undefined;

/** The app's client id names the folder every record lives under (`/pub/<clientId>/mayfly/`). */
export const app = new MayflyApp({
  clientId: (import.meta.env.VITE_CLIENT_ID as string | undefined) ?? "list.mayfly.example",
  testnet: TESTNET,
  wasm: wasmUrl,
});

/** The rules this app runs: shipped in the wasm module. */
export const RULES = "list/1";

export interface Item {
  id: string;
  text: string;
  qty: number | null;
  ticked: boolean;
}

export interface ListState {
  items: Item[];
  archived: boolean;
  parties: number;
}

export type ListBody =
  | { kind: "add"; id: string; text: string; qty?: number }
  | { kind: "edit"; id: string; text?: string; qty?: number }
  | { kind: "tick"; id: string }
  | { kind: "untick"; id: string }
  | { kind: "remove"; id: string }
  | { kind: "archive" };
