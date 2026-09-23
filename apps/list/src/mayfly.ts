// The Mayfly side: the wasm module, and the shapes it hands back (mirrors
// crates/client/src/view.rs, snake_case as serialised).

import init, {
  ChainClient,
  parseChainUrl as parseChainUrlWasm,
} from "@synonymdev/mayfly";
import wasmUrl from "@synonymdev/mayfly/pkg/mayfly_bg.wasm?url";
import { signerFromSession, storeFromPubky, type Signer, type Store } from "@synonymdev/mayfly/js/pubky-glue.js";
import type { Session } from "@synonymdev/pubky";

import { pubky } from "./pubky";

export const RULES = "list/1";

let ready: Promise<void> | undefined;

/** Load the wasm once. */
export function loadMayfly(): Promise<void> {
  ready ??= init({ module_or_path: wasmUrl }).then(() => undefined);
  return ready;
}

export interface LinkView {
  seq: number;
  round: number;
  kind: string;
  body: Record<string, unknown>;
  author: number;
  author_pubky: string;
  hash: string;
  h16: string;
  ts: number;
  confirmers: number[];
  witnessed: [number, number];
  is_final: boolean;
}

export interface CandidateView {
  hash: string;
  h16: string;
  author: number;
  kind: string;
  round: number;
  votes: number;
}

export interface OpenSeqView {
  seq: number;
  round: number;
  dead: boolean;
  candidates: CandidateView[];
  voters: [number, number[]][];
}

export interface StatusView {
  kind: "ongoing" | "stalled" | "paused" | "closed" | "abandoned";
  summary: string;
  parties: number[];
  outcome: string | null;
}

export interface AnomalyView {
  kind: string;
  against: string | null;
  against_pubky: string | null;
  seq: number | null;
  evidence: string[];
}

export interface EngagedView {
  pubky: string;
  kid: string;
  until: number;
  poll_ms: number;
  path: string;
}

export interface Arrangement {
  rules: string;
  parties: string[];
  witnesses: string[];
  confirm_quorum: number;
}

export interface ChainView {
  chain: string;
  rules: string | null;
  parties: string[];
  status: StatusView;
  is_final: boolean;
  committed: LinkView[];
  open: OpenSeqView | null;
  seats: { pubky: string; kid: string; client_id: string; paths: string[]; grant_exp: number }[];
  engaged: EngagedView[];
  anomalies: AnomalyView[];
  suspects: { owner: string; path: string }[];
  state: ListState | null;
}

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

export type ActionView =
  | { kind: "confirmed"; hash: string }
  | { kind: "reproposed"; earlier: string; link: string }
  | { kind: "passed"; hash: string }
  | { kind: "skipped"; hash: string }
  | { kind: "rejected"; link: string; reason: string }
  | { kind: "decision"; candidate: CandidateView; round: number; repropose: boolean }
  | { kind: "my_turn"; round: number }
  | { kind: "awaiting_witnesses"; have: number; of: number; want: number };

/** A member's storage and signing, from their session. */
export async function party(session: Session): Promise<{ store: Store; signer: Signer }> {
  await loadMayfly();
  return {
    store: storeFromPubky(pubky, session),
    signer: await signerFromSession(session),
  };
}

export interface ChainRef {
  chain: string;
  owner: string;
  folder: string;
}

export function parseChainUrl(url: string): ChainRef {
  return parseChainUrlWasm(url) as ChainRef;
}

export { ChainClient };
export type { Signer, Store };
