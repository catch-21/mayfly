import { useEffect, useState } from "react";
import QRCode from "qrcode";
import type { GrantAuthFlow, Session } from "@synonymdev/pubky";

import { TESTNET, startRingSignIn, testnetSignUp } from "./pubky";

export function SignIn({ onSession }: { onSession: (s: Session) => void }) {
  const [flow, setFlow] = useState<GrantAuthFlow>();
  const [qr, setQr] = useState<string>();
  const [error, setError] = useState<string>();
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);

  // Start the Ring flow on mount; the QR is the whole sign-in for a real user.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const f = await startRingSignIn();
        if (cancelled) return;
        setFlow(f);
        setQr(await QRCode.toDataURL(f.authorizationUrl, { margin: 1, width: 240 }));
        const session = await f.awaitApproval();
        if (!cancelled) onSession(session);
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [onSession]);

  const shortcut = async () => {
    setBusy(true);
    setError(undefined);
    try {
      onSession(await testnetSignUp(token.trim() || undefined));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
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
        {qr ? (
          <>
            <img className="qr" src={qr} alt="Scan with Pubky Ring to sign in" />
            <p className="dim">
              Scan with Pubky Ring, or open the link on this device:{" "}
              <a href={flow?.authorizationUrl}>pubkyauth link</a>
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
      {error && <p className="error">{error}</p>}
    </main>
  );
}
