import { useState } from "react";
import { describeError, parseChainUrl, parsePubkys, type Party } from "@synonymdev/mayfly-browser";
import { useCreateChain, useMyChains } from "@synonymdev/mayfly-browser/react";

import { RULES, app } from "./config";
import { copy, short } from "./format";

export function Home({ party, onOpen }: { party: Party; onOpen: (url: string) => void }) {
  const me = party.me;
  const lists = useMyChains(app, party);
  const creating = useCreateChain(party, RULES);
  const [members, setMembers] = useState("");
  const [witness, setWitness] = useState("");
  const [invite, setInvite] = useState("");
  const [error, setError] = useState<string>();
  const [copied, setCopied] = useState(false);

  const create = async () => {
    setError(undefined);
    try {
      // Blanks and my own pubky are dropped; a repeat or a non-pubky is refused by name, before
      // a chain is written.
      const others = parsePubkys(members, { exclude: me });
      if (others.length === 0) throw new Error("name at least one other member by pubky");
      const watchmen = parsePubkys(witness);
      if (watchmen.includes(me)) throw new Error("you cannot be your own watchman");
      onOpen(await creating.create({ parties: [me, ...others], witnesses: watchmen }));
    } catch (e) {
      setError(describeError(e));
    }
  };

  const open = () => {
    try {
      parseChainUrl(invite);
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
        {!lists.chains ? (
          <p className="dim">{lists.error ?? "Reading your homeserver…"}</p>
        ) : lists.chains.length === 0 ? (
          <p className="dim">None yet. Create one below, or open a link someone sent you.</p>
        ) : (
          <ul className="lists">
            {lists.chains.map((l) => {
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
        <button onClick={create} disabled={creating.busy}>
          {creating.busy ? "Creating…" : "Create list"}
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
