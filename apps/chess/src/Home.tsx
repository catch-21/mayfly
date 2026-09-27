import { useState } from "react";
import { describeError, isPubky, parseChainUrl, parsePubkys, type Party } from "@synonymdev/mayfly-browser";
import { useCreateChain, useMyChains } from "@synonymdev/mayfly-browser/react";

import { RULES, THINK_PRESETS, app } from "./config";
import { copy, short } from "./format";

type Colour = "white" | "black" | "random";

export function Home({ party, onOpen }: { party: Party; onOpen: (url: string) => void }) {
  const me = party.me;
  const games = useMyChains(app, party);
  const creating = useCreateChain(party, RULES);
  const [opponent, setOpponent] = useState("");
  const [colour, setColour] = useState<Colour>("random");
  const [preset, setPreset] = useState(1);
  const [witness, setWitness] = useState("");
  const [invite, setInvite] = useState("");
  const [error, setError] = useState<string>();
  const [copied, setCopied] = useState(false);

  const create = async () => {
    setError(undefined);
    try {
      const others = parsePubkys(opponent, { exclude: me });
      if (others.length !== 1) throw new Error("name exactly one opponent by pubky");
      const watchmen = witness.trim() ? parsePubkys(witness) : [];
      if (watchmen.includes(me) || watchmen.includes(others[0])) {
        throw new Error("a player cannot be the watchman");
      }
      if (watchmen.length > 1) throw new Error("name one watchman");
      const think = THINK_PRESETS[preset];
      const spec: {
        parties: string[];
        witnesses: string[];
        options: { time_control: { think_ms: number; respond_ms: number } };
        roles?: string[];
      } = {
        parties: [me, others[0]],
        witnesses: watchmen,
        options: { time_control: { think_ms: think.think_ms, respond_ms: 86_400_000 } },
      };
      if (colour === "white") spec.roles = ["white", "black"];
      if (colour === "black") spec.roles = ["black", "white"];
      onOpen(await creating.create(spec));
    } catch (e) {
      setError(describeError(e));
    }
  };

  const open = () => {
    try {
      parseChainUrl(invite);
      if (!isPubky(parseChainUrl(invite).owner)) throw new Error("bad link");
      onOpen(invite.trim());
    } catch {
      setError("that is not a game link; it looks like pubky://<owner>/pub/<app>/mayfly/chains/<id>/");
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
        <p className="dim">Give this to the person you want to play.</p>
      </section>

      <section className="card">
        <h2>Your games</h2>
        {!games.chains ? (
          <p className="dim">{games.error ?? "Reading your homeserver…"}</p>
        ) : games.chains.length === 0 ? (
          <p className="dim">None yet. Start one below, or open a link someone sent you.</p>
        ) : (
          <ul className="lists">
            {games.chains.map((g) => {
              const ref = parseChainUrl(g.url);
              return (
                <li key={g.url}>
                  <button className="link" onClick={() => onOpen(g.url)}>
                    {short(ref.chain, 5)}
                  </button>{" "}
                  <span className="dim">
                    {ref.owner === me ? "you started it" : `started by ${short(ref.owner)}`}
                    {g.finished ? " · finished" : ""}
                  </span>
                </li>
              );
            })}
          </ul>
        )}
      </section>

      <section className="card">
        <h2>New game</h2>
        <label>
          Opponent's pubky
          <input value={opponent} onChange={(e) => setOpponent(e.target.value)} spellCheck={false} />
        </label>
        <label>
          Your colour
          <select value={colour} onChange={(e) => setColour(e.target.value as Colour)}>
            <option value="random">Drawn at random, after you both join</option>
            <option value="white">White</option>
            <option value="black">Black</option>
          </select>
        </label>
        <label>
          Time for each move
          <select value={preset} onChange={(e) => setPreset(Number(e.target.value))}>
            {THINK_PRESETS.map((p, i) => (
              <option key={p.label} value={i}>
                {p.label}
              </option>
            ))}
          </select>
        </label>
        <label>
          Watchman pubky
          <input value={witness} onChange={(e) => setWitness(e.target.value)} spellCheck={false} />
        </label>
        <p className="dim">
          The watchman is what times each move: how long the player to move took, and how long the
          other took to confirm. Leave it blank and the game still plays, but the moves are ordered
          and not timed. The signer's own clock is not used.
        </p>
        <button onClick={create} disabled={creating.busy}>
          {creating.busy ? "Creating…" : "Create game"}
        </button>
      </section>

      <section className="card">
        <h2>Open a game from a link</h2>
        <div className="row">
          <input
            placeholder="pubky://…/mayfly/chains/…/"
            value={invite}
            onChange={(e) => setInvite(e.target.value)}
            spellCheck={false}
          />
          <button onClick={open}>Open</button>
        </div>
      </section>
      {error && <p className="error">{error}</p>}
    </main>
  );
}
