// The plain-data views the Mayfly module returns, mirroring `pubky_mayfly_client::view`
// (snake_case keys as in the Rust structs). Kept by hand because the wasm-bindgen types are
// `any` at the boundary. `S` is the rules state; an app names its own.

export type PartyIndex = number;

export interface TimeReading {
  witness: string;
  ms: number;
}

/** A watchman time. `ms` is set when the witnesses who answered agree; `split` lists each reading when they do not. */
export interface WitnessTime {
  ms: number | null;
  split: TimeReading[];
}

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
  /** When a watchman saw the proposal. `ts` is the author's own clock, not this. */
  observed_at: WitnessTime;
  /** When a watchman saw the quorum-completing confirmation. */
  confirmed_at: WitnessTime;
  /** How long the proposer took after the previous quorum. */
  think: WitnessTime;
  /** How long confirmation took after the proposal. */
  respond: WitnessTime;
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
  /** When the head's quorum completed. Time on the open move is `now − ready_at.ms`. */
  ready_at: WitnessTime;
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
  /** Rules roles in party order, or null where genesis set none. */
  roles: (string | null)[];
  /** Genesis options, including `time_control`. */
  options: Record<string, unknown>;
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
  | { kind: "awaiting_witnesses"; have: number; of: number; want: number }

  | { kind: "proposed"; hash: string }
  | { kind: "held_refused"; body: Body; reason: string };

export type DecisionAction = Extract<ActionView, { kind: "decision" }>;

/** Where a member stands with a chain (§8.1). */
export type Phase =
  /** Genesis has not been read yet. */
  | "loading"
  /** Genesis names parties and I am not one of them. */
  | "stranger"
  /** I am named and have not signed genesis: show the terms, offer to join. */
  | "invited"
  /** I have signed; genesis is not committed until everyone has. */
  | "waiting"
  /** Committed and ongoing. */
  | "open"
  /** Final: a close has committed and been sealed by its successor. */
  | "ended";

/** What the client derives for a page between two calls. */
export interface SessionView<B extends Body = Body> {
  chain: string;
  /** This chain at my folder: the invite URL. */
  url: string;
  me: string;
  phase: Phase;
  /** From the committed genesis, or from genesis as written before it commits. */
  parties: string[];
  my_index: number | null;
  /** Candidates of the live round only. */
  pending: CandidateView[];
  /** Proposals held until a round takes them, oldest first. */
  held: B[];
}

/** One chain a member is on. */
export interface MyChain {
  url: string;
  finished: boolean;
}

/** Which of the chain's declared folders a file was read from. */
export type FolderRole = "initiator" | "seat" | "witness";

/** One folder the reader lists. */
export interface Folder {
  role: FolderRole;
  owner: string;
  /** Absolute prefix listed, ending in `/`. */
  prefix: string;
  /** Why the listing failed, if it did. */
  error: string | null;
}

/** One listed file, decoded when it is a `.jws`. */
export interface LoadedFile {
  folder: Folder;
  /** Absolute path on the owner's homeserver. */
  path: string;
  /** Path relative to the folder prefix. */
  relative: string;
  /** The homeserver's ETag, base64url, when the listing reported one. */
  content_hash: string | null;
  /** Decoded record; absent for files that are not `.jws` or that could not be fetched. */
  record: RecordView | null;
  /** Why the bytes could not be fetched, if they could not. */
  fetch_error: string | null;
}

/** A chain as read from files: everything a page renders. */
export interface Loaded<S = unknown> {
  chain: string;
  owner: string;
  folder: string;
  /** The canonical chain URL. */
  url: string;
  /** Rules id read from genesis; `null` when genesis could not be found or read. */
  rules: string | null;
  /** Whether the registry could run `rules`. */
  rules_known: boolean;
  /** The verified chain, when verification ran and succeeded. */
  view: ChainView<S> | null;
  /** Why there is no view: no genesis yet, unknown rules, a store error. */
  view_error: string | null;
  folders: Folder[];
  files: LoadedFile[];
}

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

/** A parsed chain URL; a record link inside the chain folder parses to the same. */
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
  /** Per party, a rules role (`white`, `black`). Omit for a random colour. */
  roles?: (string | null)[];
}
