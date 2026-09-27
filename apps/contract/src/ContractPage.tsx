import { useMemo, useState } from "react";
import type { Party } from "@synonymdev/mayfly-browser";
import { useChainReader } from "@synonymdev/mayfly-browser/react";

import { contractRegistry } from "./config";
import { lineDiff } from "./diff";
import { copy, partyLabel, short } from "./format";
import type { ContractOffer, ContractOutcome, ContractState } from "./rules";
import { useContract } from "./useContract";

export function ContractPage({ party, url, onBack }: { party: Party; url: string; onBack: () => void }) {
  const chain = useContract(party, url);
  const registry = useMemo(() => contractRegistry(), []);
  const reading = chain.phase === "stranger";
  const reader = useChainReader<ContractState>(
    reading ? { store: party.store, registry, url } : undefined,
  );
  const [copied, setCopied] = useState(false);

  const share = async () => {
    setCopied(await copy(url));
    setTimeout(() => setCopied(false), 1500);
  };

  const header = (
    <header className="bar">
      <button className="link" onClick={onBack}>
        ← your contracts
      </button>
      <span className="grow" />
      <button className="small" onClick={share}>
        {copied ? "link copied" : "copy invite link"}
      </button>
    </header>
  );

  if (chain.phase === "loading") {
    return (
      <main className="narrow">
        {header}
        <p className="dim">{chain.error ?? "Reading the contract from both homeservers…"}</p>
      </main>
    );
  }

  if (reading) {
    const view = reader.loaded?.view;
    const state = view?.state ?? null;
    return (
      <main className="narrow">
        {header}
        <section className="card">
          <h2>Not your contract</h2>
          <p>
            You are not one of the two parties. This is the chain as anyone can read it
            {reader.final ? ", and it is final" : ""}.
          </p>
          {reader.error && <p className="error">{reader.error}</p>}
          {!view && !reader.error && <p className="dim">Reading the files…</p>}
          {state && <Deed state={state} />}
          {view && view.anomalies.length + view.suspects.length > 0 && (
            <Anomalies
              anomalies={view.anomalies}
              suspects={view.suspects}
              parties={view.parties}
              me={chain.me}
            />
          )}
        </section>
      </main>
    );
  }

  if (chain.phase === "invited" || chain.phase === "waiting") {
    const watchmen = chain.arrangement?.witnesses.length ?? 0;
    const mine = chain.myIndex;
    return (
      <main className="narrow">
        {header}
        <section className="card">
          {chain.phase === "waiting" ? (
            <>
              <h2>Waiting for the other party</h2>
              <p>
                {mine === 0 ? "You created this contract. " : "You have joined. "}
                Nothing can be proposed until both of you have confirmed the start.
                {mine === 0 ? " Send them the invite link." : ""}
              </p>
            </>
          ) : (
            <>
              <h2>Join this contract?</h2>
              <p>
                Joining signs your agreement to these terms: these two parties, both of whom must
                confirm every step, rules <span className="mono">contract/1</span>, and{" "}
                {watchmen
                  ? `${watchmen} ${watchmen === 1 ? "watchman" : "watchmen"} keeping time`
                  : "no watchman, so the time of the agreement is the time you each sign"}
                .
              </p>
            </>
          )}
          <Members parties={chain.parties} seated={chain.view?.seats ?? []} me={chain.me} />
          {chain.phase === "invited" && (
            <button onClick={() => void chain.join()} disabled={chain.busy}>
              {chain.busy ? "Joining…" : "Join"}
            </button>
          )}
          {chain.error && <p className="error">{chain.error}</p>}
        </section>
      </main>
    );
  }

  const state = chain.state;
  const named = chain.parties;
  const v = chain.view;
  if (!state || !v) {
    return (
      <main className="narrow">
        {header}
        <p className="dim">Reading the contract from both homeservers…</p>
      </main>
    );
  }

  const open = chain.phase === "open";
  const offer = state.offer;
  const outcome = state.outcome;
  const mine = chain.myIndex;
  const answering = open && offer !== null && mine === (offer.author === 0 ? 1 : 0);
  const anomalies = v.anomalies.filter((a) => !(a.kind === "UnconfirmedProposal" && !a.against_pubky));

  return (
    <main className="narrow">
      {header}
      {chain.decisions.length > 0 && (
        <section className="card decision">
          <h2>Needs your answer</h2>
          {chain.decisions.map((d) => (
            <CloseAsk
              key={d.action.candidate.hash}
              repropose={d.action.repropose}
              votes={d.action.candidate.votes}
              author={d.action.candidate.author}
              parties={named}
              me={chain.me}
              outcome={outcome}
              busy={chain.busy}
              onConfirm={() => void chain.confirm(d.action.candidate.hash)}
              onReject={() => void chain.reject(d.action.candidate.hash)}
              onAgain={() => void chain.repropose(d.action.candidate.hash)}
              onLetGo={() => void chain.letGo()}
            />
          ))}
        </section>
      )}

      <section className="card">
        <h1 className="title">
          Contract <span className="mono dim">{short(v.chain, 5)}</span>
        </h1>
        <Members parties={named} seated={v.seats} me={chain.me} />
        <Deed state={state} />
        {answering && offer && (
          <Answer
            offer={offer}
            busy={chain.busy}
            onAccept={() => void chain.accept(offer.id)}
            onReject={() => void chain.rejectOffer(offer.id)}
            onRevise={(text) => void chain.revise(text, offer)}
          />
        )}
        {open && offer && !answering && (
          <p className="status">
            Waiting for {partyLabel(named, offer.author === 0 ? 1 : 0, chain.me)} to accept, reject, or
            revise.
          </p>
        )}
        {open && !offer && !outcome && (
          <Compose
            label="Write the contract"
            action="Propose"
            busy={chain.busy}
            onSubmit={(text) => void chain.offer(text)}
          />
        )}
        {chain.pending.length > 0 && (
          <ul className="items">
            {chain.pending.map((k) => (
              <li key={k.hash} className="pending">
                <span className="dot" />
                <span>
                  {partyLabel(named, k.author, chain.me)}: <b>{k.kind}</b> · {k.votes}/{named.length} confirmed
                </span>
              </li>
            ))}
          </ul>
        )}
        {chain.held.length > 0 && (
          <ul className="items">
            {chain.held.map((h, i) => (
              <li key={i} className="pending">
                <span className="dot" />
                <span>
                  your {h.kind} is waiting for a round
                  {h.kind === "offer" ? `: ${h.text.slice(0, 80)}` : ""}
                </span>
              </li>
            ))}
          </ul>
        )}
        {chain.error && <p className="error">{chain.error}</p>}
        {v.engaged.length > 0 && (
          <p className="status">
            Watched by {v.engaged.length} {v.engaged.length === 1 ? "watchman" : "watchmen"}.
          </p>
        )}
      </section>

      {open && offer && (
        <section className="card">
          <h2>Walk away</h2>
          <p className="dim">
            End the negotiation with no contract. The other party must agree. Refusing that close
            withholds consent, and the offer stays open.
          </p>
          <button className="ghost" onClick={() => void chain.walkAway()} disabled={chain.busy}>
            Propose to end with no contract
          </button>
        </section>
      )}

      {open && outcome && (
        <section className="card">
          <h2>Record the ending</h2>
          <p className="dim">
            {outcome.kind === "agreed"
              ? "You have both agreed this text. A finished close records that. Refusing it is obstruction: the negotiation has already ended."
              : outcome.kind === "lapsed"
                ? "The negotiation was let go. A finished close records that there is no contract. Refusing it is obstruction."
                : "The offer was rejected. A finished close records that there is no contract. Refusing it is obstruction."}
          </p>
          <button onClick={() => void chain.record()} disabled={chain.busy}>
            {outcome.kind === "agreed" ? "Record that you both agreed" : "Record that there is no contract"}
          </button>
        </section>
      )}

      {chain.phase === "ended" && (
        <section className="card">
          <p className="status">
            This chain is finished
            {v.status.outcome ? `: ${v.status.outcome}` : ""}.
          </p>
        </section>
      )}

      {anomalies.length + v.suspects.length > 0 && (
        <Anomalies anomalies={anomalies} suspects={v.suspects} parties={named} me={chain.me} />
      )}
    </main>
  );
}

function Deed({ state }: { state: ContractState }) {
  if (state.outcome) {
    return (
      <>
        <h2>
          {state.outcome.kind === "agreed"
            ? "Agreed text"
            : state.outcome.kind === "rejected"
              ? "Rejected text"
              : "No contract"}
        </h2>
        {state.outcome.text ? (
          <pre className="deed">{state.outcome.text}</pre>
        ) : (
          <p className="dim">The negotiation ended with no contract.</p>
        )}
      </>
    );
  }
  if (!state.offer) {
    return <p className="dim">No offer yet. Either party may write the first one.</p>;
  }
  return (
    <>
      <h2>Open offer</h2>
      {state.offer.previous !== null && <Redline before={state.offer.previous} after={state.offer.text} />}
      {state.offer.previous === null && <pre className="deed">{state.offer.text}</pre>}
    </>
  );
}

function Redline({ before, after }: { before: string; after: string }) {
  return (
    <pre className="deed diff">
      {lineDiff(before, after).map((line, i) => (
        <div key={i} className={line.kind}>
          {line.kind === "add" ? "+ " : line.kind === "del" ? "- " : "  "}
          {line.text}
        </div>
      ))}
    </pre>
  );
}

function Answer({
  offer,
  busy,
  onAccept,
  onReject,
  onRevise,
}: {
  offer: ContractOffer;
  busy: boolean;
  onAccept: () => void;
  onReject: () => void;
  onRevise: (text: string) => void;
}) {
  const [draft, setDraft] = useState(offer.text);
  const changed = draft.trim().length > 0 && draft !== offer.text;
  return (
    <div className="answer">
      <p>You can accept this text, reject it outright, or send a revision.</p>
      <div className="row">
        <button onClick={onAccept} disabled={busy}>
          Accept
        </button>
        <button className="ghost" onClick={onReject} disabled={busy}>
          Reject outright
        </button>
      </div>
      <label>
        Revision
        <textarea rows={8} value={draft} onChange={(e) => setDraft(e.target.value)} disabled={busy} />
      </label>
      <button onClick={() => onRevise(draft)} disabled={busy || !changed}>
        Send revision
      </button>
    </div>
  );
}

function Compose({
  label,
  action,
  busy,
  onSubmit,
}: {
  label: string;
  action: string;
  busy: boolean;
  onSubmit: (text: string) => void;
}) {
  const [text, setText] = useState("");
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        const t = text.trim();
        if (!t) return;
        setText("");
        onSubmit(t);
      }}
    >
      <label>
        {label}
        <textarea rows={8} value={text} onChange={(e) => setText(e.target.value)} disabled={busy} />
      </label>
      <button type="submit" disabled={busy || !text.trim()}>
        {action}
      </button>
    </form>
  );
}

function CloseAsk({
  repropose,
  votes,
  author,
  parties,
  me,
  outcome,
  busy,
  onConfirm,
  onReject,
  onAgain,
  onLetGo,
}: {
  repropose: boolean;
  votes: number;
  author: number;
  parties: string[];
  me: string;
  outcome: ContractOutcome | null;
  busy: boolean;
  onConfirm: () => void;
  onReject: () => void;
  onAgain: () => void;
  onLetGo: () => void;
}) {
  const who = partyLabel(parties, author, me);
  const ending = outcome
    ? outcome.kind === "agreed"
      ? "record that you both agreed"
      : "record that there is no contract"
    : "end with no contract";
  const note = outcome
    ? "Refusing this close is obstruction: the negotiation has already ended."
    : "Refusing withholds your consent. The offer stays open.";
  if (repropose && outcome) {
    return (
      <div className="ask">
        <p>
          The negotiation has already ended. Record it, so the chain finishes.
        </p>
        <button onClick={onAgain} disabled={busy}>
          Record the ending
        </button>
      </div>
    );
  }
  if (repropose) {
    return (
      <div className="ask">
        <p>
          The proposal to <b>{ending}</b> was not confirmed. Put it forward again, or let the
          negotiation go. Letting it go ends the contract for both of you, with no contract, and the
          question is not passed on.
        </p>
        <div className="row">
          <button onClick={onAgain} disabled={busy}>
            Propose again
          </button>
          <button className="ghost" onClick={onLetGo} disabled={busy}>
            Let it go
          </button>
        </div>
      </div>
    );
  }
  return (
    <div className="ask">
      <p>
        {who === "you" ? "You propose" : `${who} proposes`} to <b>{ending}</b>. {votes} of {parties.length}{" "}
        have agreed. {note}
      </p>
      <div className="row">
        <button onClick={onConfirm} disabled={busy}>
          Agree
        </button>
        <button className="ghost" onClick={onReject} disabled={busy}>
          Refuse
        </button>
      </div>
    </div>
  );
}

function Members({ parties, seated, me }: { parties: string[]; seated: { pubky: string }[]; me: string }) {
  return (
    <p className="members">
      {parties.map((p, i) => {
        const joined = seated.some((s) => s.pubky === p);
        return (
          <span key={p} className={`chip ${joined ? "" : "faint"}`} title={p}>
            {p === me ? "you" : short(p)}
            {joined ? "" : " (not joined)"}
            {i === 0 ? " · creator" : ""}
          </span>
        );
      })}
    </p>
  );
}

function Anomalies({
  anomalies,
  suspects,
  parties,
  me,
}: {
  anomalies: { kind: string; against_pubky: string | null; seq: number | null }[];
  suspects: { owner: string; path: string }[];
  parties: string[];
  me: string;
}) {
  return (
    <section className="card warn">
      <h2>Something to look at</h2>
      <ul>
        {suspects.map((s) => (
          <li key={s.owner + s.path}>
            {short(s.owner)} holds a file whose bytes do not match its name: {s.path.split("/").pop()}
          </li>
        ))}
        {anomalies.map((a, i) => (
          <li key={i}>
            {a.kind}
            {a.against_pubky ? ` by ${partyLabel(parties, parties.indexOf(a.against_pubky), me)}` : ""}
            {a.seq != null ? ` at change ${a.seq}` : ""}
          </li>
        ))}
      </ul>
    </section>
  );
}
