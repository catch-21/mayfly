// The plain-data views the Mayfly module returns, mirroring `pubky_mayfly_client::view`
// (snake_case keys as in the Rust structs). Kept by hand because the wasm-bindgen types are
// `any` at the boundary. `S` is the rules state; an app names its own.

export type PartyIndex = number;

export interface LinkView {
  seq: number;
  round: number;
  kind: string;
  body: Record<string, unknown>;
  author: PartyIndex;
  author_pubky: string;
  hash: string;
  h16: string;
  ts: number;
  confirmers: PartyIndex[];
  witnessed: [number, number];
  is_final: boolean;
}

export interface CandidateView {
  hash: string;
  h16: string;
  author: PartyIndex;
  kind: string;
  round: number;
  votes: number;
}

export interface OpenSeqView {
  seq: number;
  /** The highest round with a vote: the dead round while `dead` is true (§6.4). */
  round: number;
  dead: boolean;
  candidates: CandidateView[];
  voters: [number, PartyIndex[]][];
}

export interface StatusView {
  kind: "ongoing" | "stalled" | "paused" | "closed" | "abandoned";
  summary: string;
  /** Who the chain waits on (stalled), or the subjects (abandoned). */
  parties: PartyIndex[];
  outcome: string | null;
}

export interface AnomalyView {
  kind: string;
  against: string | null;
  against_pubky: string | null;
  seq: number | null;
  evidence: string[];
}

export interface SeatView {
  pubky: string;
  kid: string;
  client_id: string;
  paths: string[];
  grant_exp: number;
}

export interface EngagedView {
  pubky: string;
  kid: string;
  until: number;
  poll_ms: number;
  path: string;
}

export interface SuspectView {
  owner: string;
  path: string;
}

/** Genesis terms, available before genesis commits (§8.1). */
export interface Arrangement {
  rules: string;
  parties: string[];
  witnesses: string[];
  confirm_quorum: number;
}

export interface ChainView<S = unknown> {
  chain: string;
  rules: string | null;
  /** From the committed genesis: empty until everyone has joined. Use `Arrangement` before. */
  parties: string[];
  status: StatusView;
  is_final: boolean;
  committed: LinkView[];
  open: OpenSeqView | null;
  seats: SeatView[];
  engaged: EngagedView[];
  anomalies: AnomalyView[];
  suspects: SuspectView[];
  state: S | null;
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

export type DecisionAction = Extract<ActionView, { kind: "decision" }>;

/** One decoded record file (§14). */
export interface RecordView {
  raw: string;
  hash: string;
  h16: string;
  typ: string | null;
  payload: unknown;
  claimed_key: string | null;
  signature_ok: boolean | null;
  hash_matches_etag: boolean | null;
  hash_matches_name: boolean | null;
  error: string | null;
}

/** A parsed chain URL. */
export interface ChainRef {
  chain: string;
  owner: string;
  /** The initiator's protocol folder, e.g. `/pub/list.example/mayfly/`. */
  folder: string;
  /** The canonical chain URL, `pubky://<owner><folder>chains/<chain>/`. */
  url: string;
}

/** A rules body: the `kind` names the link kind; the rest is the rules' own. */
export interface Body {
  kind: string;
  [key: string]: unknown;
}

/** What `createChain` takes; the wasm `create` spec in camelCase. */
export interface ChainSpec {
  /** Every party's pubky, the creator first. */
  parties: string[];
  /** Per party, the client id they were invited through; defaults to the creator's for all. */
  apps?: string[];
  /** Omit for unanimity. */
  confirmQuorum?: number;
  witnesses?: string[];
  recoveryDelayMs?: number;
  maxBodyBytes?: number;
  /** Rules options; `time_control: { think_ms, respond_ms }` is read by the protocol. */
  options?: Record<string, unknown>;
}
