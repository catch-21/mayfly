import { useEffect, useState } from "react";
import type { Session } from "@synonymdev/pubky";

import { copy, short } from "./format";
import { parseChainUrl } from "./mayfly";
import { createList, myLists } from "./useList";

export function Home({ session, onOpen }: { session: Session; onOpen: (url: string) => void }) {
  const me = session.info.publicKey.z32();
  const [lists, setLists] = useState<{ url: string; finished: boolean }[]>();
  const [members, setMembers] = useState("");
  const [witness, setWitness] = useState("");
  const [invite, setInvite] = useState("");
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    myLists(session)
      .then(setLists)
      .catch((e) => setError(e instanceof Error ? e.message : String(e)));
  }, [session]);

  const create = async () => {
    setBusy(true);
    setError(undefined);
    try {
      const others = members
        .split(/[\s,]+/)
        .map((s) => s.trim())
        .filter(Boolean);
      if (others.length === 0) throw new Error("name at least one other member by pubky");
      const url = await createList(session, others, witness.trim() ? [witness.trim()] : []);
      onOpen(url);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const open = () => {
    try {
      parseChainUrl(invite.trim());
      onOpen(invite.trim());
    } catch {
      setError("that is not a list link; it looks like pubky://<owner>/pub/<app>/mayfly/chains/<id>/");
    }
  };

  return (
    <main className="narrow">
      <section className="card">
        <h2>You</h2>
        <p className="mono">
          {me}{" "}
          <button
            className="small"
            onClick={async () => {
              setCopied(await copy(me));
              setTimeout(() => setCopied(false), 1500);
            }}
          >
            {copied ? "copied" : "copy"}
          </button>
        </p>
        <p className="dim">Give this to whoever is creating a list you should be on.</p>
      </section>

      <section className="card">
        <h2>Your lists</h2>
        {!lists ? (
          <p className="dim">Reading your homeserver…</p>
        ) : lists.length === 0 ? (
          <p className="dim">None yet. Create one below, or open a link someone sent you.</p>
        ) : (
          <ul className="lists">
            {lists.map((l) => {
              const ref = parseChainUrl(l.url);
              return (
                <li key={l.url}>
                  <button className="link" onClick={() => onOpen(l.url)}>
                    {short(ref.chain, 5)}
                  </button>{" "}
                  <span className="dim">
                    {ref.owner === me ? "created by you" : `created by ${short(ref.owner)}`}
                    {l.finished ? " · finished" : ""}
                  </span>
                </li>
              );
            })}
          </ul>
        )}
      </section>

      <section className="card">
        <h2>New list</h2>
        <label>
          Other members, by pubky (one per line or comma-separated)
          <textarea rows={3} value={members} onChange={(e) => setMembers(e.target.value)} />
        </label>
        <label>
          Watchman pubky (optional): an impartial timekeeper that receipts every change
          <input value={witness} onChange={(e) => setWitness(e.target.value)} />
        </label>
        <p className="dim">
          Every member must confirm each change (unanimity). You will be the first member.
        </p>
        <button onClick={create} disabled={busy}>
          {busy ? "Creating…" : "Create list"}
        </button>
      </section>

      <section className="card">
        <h2>Open a list from a link</h2>
        <div className="row">
          <input
            placeholder="pubky://…/mayfly/chains/…/"
            value={invite}
            onChange={(e) => setInvite(e.target.value)}
          />
          <button onClick={open}>Open</button>
        </div>
      </section>
      {error && <p className="error">{error}</p>}
    </main>
  );
}
