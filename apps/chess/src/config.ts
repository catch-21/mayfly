// This app on this network. The rules are `chess/1`, shipped in the wasm module.
// Shakmaty decides which moves are legal; this file only names the app.

import { MayflyApp } from "@synonymdev/mayfly-browser";
import wasmUrl from "@synonymdev/mayfly/pkg/mayfly_bg.wasm?url";

/** Testnet flavour: `VITE_TESTNET=true` at build time. */
export const TESTNET = import.meta.env.VITE_TESTNET === "true";

/** The viewer, for "open in viewer" links; `undefined` hides them. */
export const VIEWER_URL = import.meta.env.VITE_VIEWER_URL as string | undefined;

/** The app's client id names the folder every record lives under (`/pub/<clientId>/mayfly/`). */
export const app = new MayflyApp({
  clientId: (import.meta.env.VITE_CLIENT_ID as string | undefined) ?? "chess.mayfly.example",
  testnet: TESTNET,
  wasm: wasmUrl,
});

/** The rules this app runs: shipped in the wasm module. */
export const RULES = "chess/1";

export interface ChessState {
  initial_fen: string;
  moves: string[];
  white: number;
  black: number;
  parties: string[];
  draw_offer: number | null;
  result: string | null;
}

/** Think-time presets, in milliseconds. Responding has a day, because confirming is the client's job. */
export const THINK_PRESETS = [
  { label: "1 day", think_ms: 86_400_000 },
  { label: "3 days", think_ms: 3 * 86_400_000 },
  { label: "7 days", think_ms: 7 * 86_400_000 },
] as const;
