import { useState } from "react";
import type { CandidateView, Party } from "@synonymdev/mayfly-browser";

import { VIEWER_URL } from "./config";
import { copy, partyLabel, short } from "./format";
import { useList } from "./useList";

export function ListPage({ party, url, onBack }: { party: Party; url: string; onBack: () => void }) {
  const list = useList(party, url);
  const [text, setText] = useState("");
  const [editing, setEditing] = useState<string>();
  const [draft, setDraft] = useState("");
  const [copied, setCopied] = useState(false);
  const v = list.view;
  const named = list.parties;

  const share = async () => {
    setCopied(await copy(url));
    setTimeout(() => setCopied(false), 1500);
  };

  const header = (
    <header className="bar">
      <button className="link" onClick={onBack}>
        ← your lists
      </button>
      <span className="grow" />
      <button className="small" onClick={share}>
        {copied ? "link copied" : "copy invite link"}
      </button>
      {VIEWER_URL && (
        <a className="small button" href={`${VIEWER_URL}#${encodeURIComponent(url)}`} target="_blank" rel="noreferrer">
          open in viewer
        </a>
      )}
    </header>
  );

  if (list.phase === "loading") {
    return (
      <main className="narrow">
        {header}
        <p className="dim">{list.error ?? "Reading the list from its members' homeservers…"}</p>
      </main>
    );
  }

  if (list.phase === "stranger") {
    return (
      <main className="narrow">
        {header}
        <section className="card">
          <h2>Not your list</h2>
          <p>
            This list names {named.length} members and you are not one of them. You can still
            read it{VIEWER_URL ? " in the viewer" : ""}.
          </p>
          <Members parties={named} seated={v?.seats ?? []} me={list.me} />
        </section>
      </main>
    );
  }

  if (list.phase === "invited" || list.phase === "waiting") {
    const watchmen = list.arrangement?.witnesses.length ?? v?.engaged.length ?? 0;
    const mine = list.myIndex;
    return (
      <main className="narrow">
        {header}
        <section className="card">
          {list.phase === "waiting" ? (
            <>
              <h2>Waiting for the others</h2>
              <p>
                {mine === 0 ? "You created this list. " : "You have joined. "}
                Nothing can be added until every member has joined and confirmed the start.
                {mine === 0 ? " Send them the invite link." : ""}
              </p>
            </>
          ) : (
            <>
              <h2>Join this list?</h2>
              <p>
                Joining signs your agreement to the list as it was set up: these members, every
                one of whom must confirm each change, and{" "}
                {watchmen
                  ? `${watchmen} ${watchmen === 1 ? "watchman" : "watchmen"} keeping time`
                  : "no watchman"}
                . Your changes will be signed in your name and kept on your homeserver.
              </p>
            </>
          )}
          <Members parties={named} seated={v?.seats ?? []} me={list.me} />
          {list.phase === "invited" && (
            <button onClick={() => void list.join()} disabled={list.busy}>
              {list.busy ? "Joining…" : "Join"}
            </button>
          )}
          {list.error && <p className="error">{list.error}</p>}
        </section>
      </main>
    );
  }

  if (!v) {
    return (
      <main className="narrow">
        {header}
        <p className="dim">Reading the list from its members' homeservers…</p>
      </main>
    );
  }

  const items = list.items;
  const closed = list.phase === "ended" || list.state?.archived;
  const waiting = v.status.kind === "stalled" ? v.status.parties : [];
  // A proposal nobody has confirmed yet is what the pending rows already show; the verifier
  // records it as an anomaly attributed to nobody. Only attributed anomalies, and tampered
  // mirrors, are something to look at.
  const anomalies = v.anomalies.filter((a) => !(a.kind === "UnconfirmedProposal" && !a.against_pubky));

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    const t = text.trim();
    if (!t) return;
    setText("");
    void list.add(t);
  };

  const startEdit = (id: string, current: string) => {
    setEditing(id);
    setDraft(current);
  };
  const saveEdit = () => {
    const id = editing;
    const t = draft.trim();
    setEditing(undefined);
    if (!id || !t) return;
    const current = items.find((it) => it.id === id)?.text;
    if (t === current) return;
    void list.edit(id, t);
  };

  return (
    <main className="narrow">
      {header}
      <section className="card">
        <h1 className="title">
          Shopping list <span className="mono dim">{short(v.chain, 5)}</span>
        </h1>
        <Members parties={named} seated={v.seats} me={list.me} />
        <ul className="items">
          {items.map((it) =>
            editing === it.id ? (
              <li key={it.id} className="editing">
                <form
                  className="row"
                  onSubmit={(e) => {
                    e.preventDefault();
                    saveEdit();
                  }}
                >
                  <input
                    autoFocus
                    aria-label="Edit item"
                    value={draft}
                    onChange={(e) => setDraft(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Escape") setEditing(undefined);
                    }}
                    disabled={list.busy}
                  />
                  <button type="submit" disabled={list.busy || !draft.trim() || draft.trim() === it.text}>
                    Save
                  </button>
                  <button type="button" className="ghost" onClick={() => setEditing(undefined)}>
                    Cancel
                  </button>
                </form>
              </li>
            ) : (
              <li key={it.id} className={it.ticked ? "ticked" : ""}>
                <label>
                  <input
                    type="checkbox"
                    checked={it.ticked}
                    disabled={!!closed || list.busy}
                    onChange={(e) => void list.tick(it.id, e.target.checked)}
                  />
                  <span>{it.text}</span>
                </label>
                <button
                  className="small ghost"
                  title="edit"
                  aria-label={`edit ${it.text}`}
                  disabled={!!closed || list.busy}
                  onClick={() => startEdit(it.id, it.text)}
                >
                  ✎
                </button>
                <button
                  className="small ghost"
                  title="remove"
                  aria-label={`remove ${it.text}`}
                  disabled={!!closed || list.busy}
                  onClick={() => void list.remove(it.id)}
                >
                  ×
                </button>
              </li>
            ),
          )}
          {list.pending.map((k) => (
            <li key={k.hash} className="pending">
              <span className="dot" />
              <span>
                <Pending k={k} parties={named} me={list.me} />
              </span>
            </li>
          ))}
          {list.held.map((h) => (
            <li key={`held-${h.kind === "add" ? h.id : h.kind}`} className="pending">
              <span className="dot" />
              <span>
                you: <b>{h.kind}</b> {h.kind === "add" ? h.text : ""} · waiting for your turn
              </span>
            </li>
          ))}
          {items.length === 0 && list.pending.length === 0 && list.held.length === 0 && (
            <li className="dim">Nothing on the list yet.</li>
          )}
        </ul>
        {!closed && (
          <form className="row" onSubmit={submit}>
            <input
              placeholder="Add an item"
              value={text}
              onChange={(e) => setText(e.target.value)}
              disabled={list.busy}
            />
            <button type="submit" disabled={list.busy || !text.trim()}>
              Add
            </button>
          </form>
        )}
        <p className="status">
          {closed ? (
            <>This list is {v.is_final ? "closed" : "archived"}.</>
          ) : waiting.length ? (
            <>Waiting for {waiting.map((p) => partyLabel(v.parties, p, list.me)).join(", ")} to confirm.</>
          ) : (
            <>
              {Math.max(0, v.committed.length - 1)}{" "}
              {v.committed.length - 1 === 1 ? "change" : "changes"}, all confirmed.
            </>
          )}
          {v.engaged.length > 0 && (
            <>
              {" "}
              Watched by {v.engaged.length} {v.engaged.length === 1 ? "watchman" : "watchmen"}; last change witnessed{" "}
              {v.committed.at(-1)?.witnessed.join("/") ?? "0/0"}.
            </>
          )}
        </p>
        {list.error && <p className="error">{list.error}</p>}
      </section>

      {list.decisions.length > 0 && (
        <section className="card decision">
          <h2>Needs your answer</h2>
          {list.decisions.map((d) => {
            const k = d.action.candidate;
            const who = partyLabel(named, k.author, list.me);
            const verb = k.kind === "close" ? "close this list for good" : `${k.kind} this list`;
            if (d.action.repropose) {
              // The round died without confirming it, and I am the one to propose next.
              return (
                <div key={k.hash} className="ask">
                  <p>
                    {who === "you" ? "Your" : `${who}'s`} proposal to <b>{verb}</b> was not confirmed
                    in that round. It is your turn: put it forward again, or let it go.
                  </p>
                  <div className="row">
                    <button onClick={() => void list.repropose(k.hash)} disabled={list.busy}>
                      Propose again
                    </button>
                    <button className="ghost" onClick={() => void list.pass()} disabled={list.busy}>
                      Let it go
                    </button>
                  </div>
                </div>
              );
            }
            return (
              <div key={k.hash} className="ask">
                <p>
                  {who === "you" ? "You propose" : `${who} proposes`} to <b>{verb}</b>. {k.votes} of{" "}
                  {named.length} have agreed.
                </p>
                <div className="row">
                  <button onClick={() => void list.confirm(k.hash)} disabled={list.busy}>
                    Agree
                  </button>
                  <button className="ghost" onClick={() => void list.reject(k.hash)} disabled={list.busy}>
                    Refuse
                  </button>
                </div>
              </div>
            );
          })}
        </section>
      )}

      {!closed && (
        <section className="card">
          <h2>Finish</h2>
          <p className="dim">
            Archiving keeps the list but stops changes. Closing ends the chain; every member must
            agree.
          </p>
          <div className="row">
            <button className="ghost" onClick={() => void list.archive()} disabled={list.busy}>
              Archive
            </button>
            <button className="ghost" onClick={() => void list.close()} disabled={list.busy}>
              Propose to close
            </button>
          </div>
        </section>
      )}

      {anomalies.length + v.suspects.length > 0 && (
        <section className="card warn">
          <h2>Something to look at</h2>
          <ul>
            {v.suspects.map((s) => (
              <li key={s.owner + s.path}>
                {short(s.owner)} holds a file whose bytes do not match its name: {s.path.split("/").pop()}
              </li>
            ))}
            {anomalies.map((a, i) => (
              <li key={i}>
                {a.kind}
                {a.against_pubky ? ` by ${partyLabel(named, named.indexOf(a.against_pubky), list.me)}` : ""}
                {a.seq != null ? ` at change ${a.seq}` : ""}
              </li>
            ))}
          </ul>
          {VIEWER_URL && <p className="dim">The viewer shows the evidence file by file.</p>}
        </section>
      )}
    </main>
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

function Pending({ k, parties, me }: { k: CandidateView; parties: string[]; me: string }) {
  const who = partyLabel(parties, k.author, me);
  return (
    <>
      {who}: <b>{k.kind}</b> · {k.votes}/{parties.length} confirmed
    </>
  );
}
