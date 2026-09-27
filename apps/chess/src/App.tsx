import { useEffect, useState } from "react";
import { useSession } from "@synonymdev/mayfly-browser/react";

import { Home } from "./Home";
import { GamePage } from "./GamePage";
import { SignIn } from "./SignIn";
import { TESTNET, app } from "./config";
import { short } from "./format";

function gameFromHash(): string | undefined {
  const h = decodeURIComponent(location.hash.replace(/^#/, ""));
  return h.startsWith("pubky://") ? h : undefined;
}

export function App() {
  const { session, party, error, signIn, signOut } = useSession(app);
  const [url, setUrl] = useState<string | undefined>(gameFromHash);

  useEffect(() => {
    const onHash = () => setUrl(gameFromHash());
    addEventListener("hashchange", onHash);
    return () => removeEventListener("hashchange", onHash);
  }, []);

  const open = (u: string) => {
    location.hash = encodeURIComponent(u);
    setUrl(u);
  };
  const back = () => {
    history.pushState(null, "", location.pathname);
    setUrl(undefined);
  };

  if (error) return <main className="narrow error">{error}</main>;
  if (session === undefined) return <main className="narrow dim">Loading…</main>;
  if (!session) return <SignIn onSession={signIn} />;
  if (!party) return <main className="narrow dim">Preparing your signing key…</main>;

  return (
    <>
      <nav className="top">
        <span className="brand">Mayfly chess{TESTNET ? " · testnet" : ""}</span>
        <span className="grow" />
        <span className="mono dim" title={party.me}>
          {short(party.me)}
        </span>
        <button className="small ghost" onClick={() => void signOut()}>
          sign out
        </button>
      </nav>
      {url ? <GamePage party={party} url={url} onBack={back} /> : <Home party={party} onOpen={open} />}
    </>
  );
}
