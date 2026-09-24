// The Pubky side: which network, how a user signs in, and where the session is kept.
//
// Two ways in. The real one is a grant auth flow approved in Pubky Ring: the app shows a
// `pubkyauth://` URL as a QR code, the user's signing device approves it, and the SDK hands
// back a grant-backed session whose client key is what Mayfly signs records with (§5). The
// other is a testnet developer shortcut: a random keypair signed up straight onto the local
// homeserver. Both end in the same `Session`, which the SDK's browser store keeps in
// IndexedDB across reloads.

import {
  AuthFlowKind,
  Keypair,
  Pubky,
  PublicKey,
  type Capabilities,
  type GrantAuthFlow,
  type Session,
} from "@synonymdev/pubky";
import QRCode from "qrcode";
import { signerFromSession, storeFromPubky } from "@synonymdev/mayfly/js/pubky-glue.js";

import { loadMayfly, type Signer, type Store } from "./mayfly.js";

/** The static testnet's homeserver, for the developer shortcut. */
export const TESTNET_HOMESERVER = "8pinxxgqs41n4aididenw5apqp1urfmzdztr8jt4abrkdn435ewo";

/** The testnet's HTTP relay for auth flows; mainnet uses the SDK default. */
const TESTNET_HTTP_RELAY = "http://localhost:15412/inbox";

export interface MayflyAppOptions {
  /** Names the folder every record lives under: `/pub/<clientId>/mayfly/`. */
  clientId: string;
  /** `true` for the local testnet (pkarr relay `localhost:15411`, homeserver above). */
  testnet?: boolean;
  /** Where the wasm bytes are, when the bundler needs telling (Vite: `?url` import). */
  wasm?: Parameters<typeof loadMayfly>[0];
}

/** A signed-in member: their session, and the store and signer the client wants. */
export interface Party {
  session: Session;
  me: string;
  store: Store;
  signer: Signer;
}

/**
 * One app on one network. Everything a page does with Pubky — sign in, remember, sign out,
 * read as a bystander — goes through here.
 */
export class MayflyApp {
  readonly clientId: string;
  readonly testnet: boolean;
  readonly pubky: Pubky;
  private readonly wasm: MayflyAppOptions["wasm"];

  constructor(options: MayflyAppOptions) {
    this.clientId = options.clientId;
    this.testnet = options.testnet ?? false;
    this.pubky = this.testnet ? Pubky.testnet() : new Pubky();
    this.wasm = options.wasm;
  }

  /** The protocol folder under this app: `/pub/<clientId>/mayfly/`. */
  get folder(): string {
    return `/pub/${this.clientId}/mayfly/`;
  }

  /** Load the wasm module; every other call does this itself. */
  ready(): Promise<void> {
    return loadMayfly(this.wasm);
  }

  /** What a grant to this app may touch: its own folder, read and write. */
  get capabilities(): Capabilities {
    return `/pub/${this.clientId}/:rw`;
  }

  /** Start a Ring sign-in; show the QR, then `await flow.awaitApproval()`. */
  startRingSignIn(): Promise<GrantAuthFlow> {
    return this.pubky.startGrantAuthFlow(this.capabilities, AuthFlowKind.signin(), {
      clientId: this.clientId,
      relay: this.testnet ? TESTNET_HTTP_RELAY : null,
    });
  }

  /** The sign-in QR as a data URL. */
  async qrDataUrl(flow: GrantAuthFlow, width = 240): Promise<string> {
    return QRCode.toDataURL(flow.authorizationUrl, { margin: 1, width });
  }

  /**
   * Testnet shortcut: a fresh identity signed up on the local homeserver. `signupToken` is
   * needed when the homeserver runs with `signup_mode = "token_required"`; the compose and
   * `cargo run -p pubky-testnet` testnets accept none.
   */
  async testnetSignUp(signupToken?: string): Promise<Session> {
    if (!this.testnet) throw new Error("the sign-up shortcut is for the testnet only");
    const keypair = Keypair.random();
    const signer = this.pubky.signer(keypair);
    await signer.signup(PublicKey.from(TESTNET_HOMESERVER), signupToken || null);
    return signer.signin(this.clientId);
  }

  /** Keep the session for the next visit. */
  async remember(session: Session): Promise<void> {
    const store = this.pubky.browserSessionStore;
    if (!(await store.isAvailable())) return;
    await store.save(session);
  }

  /** The most recently saved session for this app, if any. A stale one is dropped. */
  async restore(): Promise<Session | undefined> {
    const store = this.pubky.browserSessionStore;
    if (!(await store.isAvailable())) return undefined;
    const saved = await store.list();
    const last = saved.filter((s) => s.clientId === this.clientId).at(-1);
    if (!last) return undefined;
    try {
      return await store.restore(last.id);
    } catch {
      await store.remove(last.id);
      return undefined;
    }
  }

  /** Sign out everywhere this app remembers. */
  async forget(session?: Session): Promise<void> {
    const store = this.pubky.browserSessionStore;
    if (await store.isAvailable()) {
      for (const s of await store.list()) {
        if (s.clientId === this.clientId) await store.remove(s.id);
      }
    }
    if (session) {
      try {
        await session.signout();
      } catch {
        // The homeserver may already have forgotten it; the local record is gone either way.
      }
    }
  }

  /** A member's storage and signing, from their session. */
  async party(session: Session): Promise<Party> {
    await this.ready();
    return {
      session,
      me: session.info.publicKey.z32(),
      store: storeFromPubky(this.pubky, session),
      signer: await signerFromSession(session),
    };
  }

  /** A read-only store: what a viewer or a verifier uses. */
  async readOnlyStore(): Promise<Store> {
    await this.ready();
    return storeFromPubky(this.pubky);
  }
}
