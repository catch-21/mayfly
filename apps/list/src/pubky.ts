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

/** Testnet flavour: `VITE_TESTNET=true` at build time. */
export const TESTNET = import.meta.env.VITE_TESTNET === "true";

/** The app's client id: names the folder every record lives under (`/pub/<clientId>/mayfly/`). */
export const CLIENT_ID = (import.meta.env.VITE_CLIENT_ID as string | undefined) ?? "list.mayfly.example";

/** The viewer (phase G), for "open in the viewer" links; `undefined` hides them. */
export const VIEWER_URL = import.meta.env.VITE_VIEWER_URL as string | undefined;

/** The static testnet's homeserver, for the developer shortcut. */
export const TESTNET_HOMESERVER = "8pinxxgqs41n4aididenw5apqp1urfmzdztr8jt4abrkdn435ewo";

/** The testnet's HTTP relay for auth flows; mainnet uses the SDK default. */
const TESTNET_HTTP_RELAY = "http://localhost:15412/inbox";

export const pubky: Pubky = TESTNET ? Pubky.testnet() : new Pubky();

/** What a grant to this app may touch: its own folder, read and write. */
const CAPABILITIES: Capabilities = `/pub/${CLIENT_ID}/:rw`;

/** Start a Ring sign-in; show `flow.authorizationUrl`, then `await flow.awaitApproval()`. */
export async function startRingSignIn(): Promise<GrantAuthFlow> {
  return pubky.startGrantAuthFlow(CAPABILITIES, AuthFlowKind.signin(), {
    clientId: CLIENT_ID,
    relay: TESTNET ? TESTNET_HTTP_RELAY : null,
  });
}

/**
 * Testnet shortcut: a fresh identity signed up on the local homeserver. `signupToken` is
 * needed when the homeserver runs with `signup_mode = "token_required"`; the compose and
 * `cargo run -p pubky-testnet` testnets accept none.
 */
export async function testnetSignUp(signupToken?: string): Promise<Session> {
  const keypair = Keypair.random();
  const signer = pubky.signer(keypair);
  await signer.signup(PublicKey.from(TESTNET_HOMESERVER), signupToken || null);
  return signer.signin(CLIENT_ID);
}

/** Keep the session for the next visit. */
export async function remember(session: Session): Promise<void> {
  const store = pubky.browserSessionStore;
  if (!(await store.isAvailable())) return;
  await store.save(session);
}

/** The most recently saved session for this app, if any. */
export async function restore(): Promise<Session | undefined> {
  const store = pubky.browserSessionStore;
  if (!(await store.isAvailable())) return undefined;
  const saved = await store.list();
  const mine = saved.filter((s) => s.clientId === CLIENT_ID);
  const last = mine.at(-1);
  if (!last) return undefined;
  try {
    return await store.restore(last.id);
  } catch {
    await store.remove(last.id);
    return undefined;
  }
}

/** Sign out everywhere this app remembers. */
export async function forget(session?: Session): Promise<void> {
  const store = pubky.browserSessionStore;
  if (await store.isAvailable()) {
    for (const s of await store.list()) {
      if (s.clientId === CLIENT_ID) await store.remove(s.id);
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
