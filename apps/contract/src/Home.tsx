import { useState } from "react";
import { describeError, parseChainUrl, parsePubkys, type Party } from "@synonymdev/mayfly-browser";
import { useCreateChain, useMyChains } from "@synonymdev/mayfly-browser/react";

import { app, contractRules } from "./config";
import { copy, short } from "./format";

export function Home({ party, onOpen }: { party: Party; onOpen: (url: string) => void }) {
  const me = party.me;
  const contracts = useMyChains(app, party);
  const creating = useCreateChain(party, contractRules);
  const [other, setOther] = useState("");
  const [witness, setWitness] = useState("");
  const [invite, setInvite] = useState("");
  const [error, setError] = useState<string>();
  const [copied, setCopied] = useState(false);

  const create = async () => {
    setError(undefined);
    try {
      const others = parsePubkys(other, { exclude: me });
      if (others.length !== 1) throw new Error("name exactly one other party by pubky");
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
      setError("that is not a contract link; it looks like pubky://<owner>/pub/<app>/mayfly/chains/<id>/");
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
        <p className="dim">Give this to the person who is creating a contract with you.</p>
      </section>

      <section className="card">
        <h2>Your contracts</h2>
        {!contracts.chains ? (
          <p className="dim">{contracts.error ?? "Reading your homeserver…"}</p>
        ) : contracts.chains.length === 0 ? (
          <p className="dim">None yet. Create one below, or open a link someone sent you.</p>
        ) : (
          <ul className="lists">
            {contracts.chains.map((c) => {
              const ref = parseChainUrl(c.url);
              return (
                <li key={c.url}>
                  <button className="link" onClick={() => onOpen(c.url)}>
                    {short(ref.chain, 5)}
                  </button>{" "}
                  <span className="dim">
                    {ref.owner === me ? "created by you" : `created by ${short(ref.owner)}`}
                    {c.finished ? " · finished" : ""}
                  </span>
                </li>
              );
            })}
          </ul>
        )}
      </section>

      <section className="card">
        <h2>New contract</h2>
        <label>
          The other party, by pubky
          <input value={other} onChange={(e) => setOther(e.target.value)} />
        </label>
        <label>
          Watchman pubky (optional): an impartial timekeeper that receipts every change
          <input value={witness} onChange={(e) => setWitness(e.target.value)} />
        </label>
        <p className="dim">
          Both of you must confirm every step. A watchman is optional; without one, the time of
          the agreement is the time you each signed.
        </p>
        <button onClick={create} disabled={creating.busy}>
          {creating.busy ? "Creating…" : "Create contract"}
        </button>
      </section>

      <section className="card">
        <h2>Open a contract from a link</h2>
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
