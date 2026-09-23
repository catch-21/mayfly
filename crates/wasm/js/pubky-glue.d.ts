import type { Pubky, Session } from "@synonymdev/pubky";

/** One file as listed by a store. */
export interface Listed {
  /** Absolute path, e.g. `/pub/list.example/mayfly/chains/<id>/links/00000007-<h16>.jws`. */
  path: string;
  /** `BLAKE3(bytes)` in any of the three spellings of §6 (the homeserver's `ETag`), if known. */
  contentHash?: string;
}

/** Read-anyone, write-me storage, as the Mayfly module expects it. */
export interface Store {
  /** My pubky (z32); `""` for a read-only store. */
  me: string;
  list(owner: string, prefix: string): Promise<Listed[]>;
  get(owner: string, path: string): Promise<Uint8Array | null | undefined>;
  put(path: string, bytes: Uint8Array): Promise<void>;
  delete(path: string): Promise<void>;
}

/** Who signs, as what, for which app. */
export interface Signer {
  pubky: string;
  kid: string;
  clientId: string;
  grantJws: string;
  signJws(typ: string, claims: unknown): Promise<string> | string;
}

export function storeFromPubky(pubky: Pubky, session?: Session): Store;
export function signerFromSession(session: Session): Promise<Signer>;
