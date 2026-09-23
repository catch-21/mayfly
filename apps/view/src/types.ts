// The plain-data views the Mayfly module returns, mirroring `pubky_mayfly_client::view`
// (snake_case keys as in the Rust structs). Kept by hand because the wasm-bindgen types are
// `any` at the boundary.

export type PartyIndex = number;

export interface LinkView {
  seq: number;
  round: number;
  kind: string;
  body: unknown;
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
  round: number;
  dead: boolean;
  candidates: CandidateView[];
  voters: [number, PartyIndex[]][];
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

export interface StatusView {
  kind: "ongoing" | "stalled" | "paused" | "closed" | "abandoned";
  summary: string;
  parties: PartyIndex[];
  outcome: string | null;
}

export interface SuspectView {
  owner: string;
  path: string;
}

export interface ChainView {
  chain: string;
  rules: string | null;
  parties: string[];
  status: StatusView;
  is_final: boolean;
  committed: LinkView[];
  open: OpenSeqView | null;
  seats: SeatView[];
  engaged: EngagedView[];
  anomalies: AnomalyView[];
  suspects: SuspectView[];
  state: unknown | null;
}

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

/** Which of the chain's declared folders a file was read from. */
export type FolderRole = "initiator" | "seat" | "witness";

/** One folder the viewer lists. */
export interface Folder {
  role: FolderRole;
  /** The folder owner's pubky. */
  owner: string;
  /** Absolute prefix listed, ending in `/`. */
  prefix: string;
  /** Why the listing failed, if it did. */
  error?: string;
}

/** One listed file, decoded when it is a `.jws`. */
export interface LoadedFile {
  folder: Folder;
  /** Absolute path on the owner's homeserver. */
  path: string;
  /** Path relative to the folder prefix. */
  relative: string;
  /** The homeserver's ETag, when the listing reported one. */
  contentHash?: string;
  /** Decoded record; absent for files that are not `.jws` or that could not be fetched. */
  record?: RecordView;
  /** Why the bytes could not be fetched, if they could not. */
  fetchError?: string;
}

/** The `list/1` state, for the checklist rendering. */
export interface ListState {
  items: { id: string; text: string; qty?: number | null; ticked: boolean }[];
  archived: boolean;
  parties: number;
}
