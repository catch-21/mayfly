import { useState } from "react";
import type { Session } from "@synonymdev/pubky";
import { describeError } from "@synonymdev/mayfly-browser";
import { useRingSignIn } from "@synonymdev/mayfly-browser/react";

import { TESTNET, app } from "./config";

export function SignIn({ onSession }: { onSession: (s: Session) => void }) {
  // The Ring flow starts on mount; the QR is the whole sign-in for a real user.
  const ring = useRingSignIn(app, onSession);
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();

  const shortcut = async () => {
    setBusy(true);
    setError(undefined);
    try {
      onSession(await app.testnetSignUp(token.trim() || undefined));
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <main className="narrow">
      <h1>Mayfly list</h1>
      <p className="lead">
        A shopping list shared by a few people. Every change is signed by whoever made it and
        confirmed by the others; anyone with the link can check the whole history.
      </p>
      <section className="card">
        <h2>Sign in with Pubky Ring</h2>
        {ring.qr ? (
          <>
            <img className="qr" src={ring.qr} alt="Scan with Pubky Ring to sign in" />
            <p className="dim">
              Scan with Pubky Ring, or open the link on this device:{" "}
              <a href={ring.flow?.authorizationUrl}>pubkyauth link</a>
            </p>
          </>
        ) : (
          <p className="dim">Preparing the sign-in request…</p>
        )}
      </section>
      {TESTNET && (
        <section className="card">
          <h2>Testnet shortcut</h2>
          <p className="dim">
            A fresh identity signed straight up on the local testnet homeserver. Leave the
            token blank unless the homeserver requires one.
          </p>
          <div className="row">
            <input
              placeholder="signup token (optional)"
              value={token}
              onChange={(e) => setToken(e.target.value)}
            />
            <button onClick={shortcut} disabled={busy}>
              {busy ? "Signing up…" : "New testnet identity"}
            </button>
          </div>
        </section>
      )}
      {(error ?? ring.error) && <p className="error">{error ?? ring.error}</p>}
    </main>
  );
}
