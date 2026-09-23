import { useCallback, useEffect, useState } from "react";
import type { Session } from "@synonymdev/pubky";

import { Home } from "./Home";
import { ListPage } from "./ListPage";
import { SignIn } from "./SignIn";
import { loadMayfly } from "./mayfly";
import { TESTNET, forget, remember, restore } from "./pubky";
import { short } from "./format";

/** The open list lives in the hash, so a list link survives a reload and can be shared. */
function listFromHash(): string | undefined {
  const h = decodeURIComponent(location.hash.replace(/^#/, ""));
  return h.startsWith("pubky://") ? h : undefined;
}

export function App() {
  const [session, setSession] = useState<Session | null>();
  const [url, setUrl] = useState<string | undefined>(listFromHash);
  const [fatal, setFatal] = useState<string>();

  useEffect(() => {
    Promise.all([loadMayfly(), restore()])
      .then(([, s]) => setSession(s ?? null))
      .catch((e) => setFatal(e instanceof Error ? e.message : String(e)));
    const onHash = () => setUrl(listFromHash());
    addEventListener("hashchange", onHash);
    return () => removeEventListener("hashchange", onHash);
  }, []);

  const onSession = useCallback(async (s: Session) => {
    setSession(s);
    await remember(s);
  }, []);

  const open = (u: string) => {
    location.hash = encodeURIComponent(u);
    setUrl(u);
  };
  const back = () => {
    history.pushState(null, "", location.pathname);
    setUrl(undefined);
  };
  const signOut = async () => {
    await forget(session ?? undefined);
    setSession(null);
  };

  if (fatal) return <main className="narrow error">{fatal}</main>;
  if (session === undefined) return <main className="narrow dim">Loading…</main>;
  if (!session) return <SignIn onSession={onSession} />;

  return (
    <>
      <nav className="top">
        <span className="brand">Mayfly list{TESTNET ? " · testnet" : ""}</span>
        <span className="grow" />
        <span className="mono dim" title={session.info.publicKey.z32()}>
          {short(session.info.publicKey.z32())}
        </span>
        <button className="small ghost" onClick={signOut}>
          sign out
        </button>
      </nav>
      {url ? <ListPage session={session} url={url} onBack={back} /> : <Home session={session} onOpen={open} />}
    </>
  );
}
