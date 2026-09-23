import { useState } from "react";
import type { Session } from "@synonymdev/pubky";

import { copy, partyLabel, short } from "./format";
import type { CandidateView, ChainView } from "./mayfly";
import { VIEWER_URL } from "./pubky";
import { useList } from "./useList";

export function ListPage({ session, url, onBack }: { session: Session; url: string; onBack: () => void }) {
  const list = useList(session, url);
  const [text, setText] = useState("");
  const [copied, setCopied] = useState(false);
  const v = list.view;
  // Genesis is not committed until every member confirms it, so the parties come from the
  // arrangement until then (§8.1).
  const named = v && v.parties.length > 0 ? v.parties : (list.arrangement?.parties ?? []);

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

  if (named.length === 0) {
    return (
      <main className="narrow">
        {header}
        <p className="dim">{list.error ?? "Reading the list from its members' homeservers…"}</p>
      </main>
    );
  }

  const mine = named.indexOf(list.me);

  if (mine < 0) {
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

  if (!list.seated) {
    const watchmen = list.arrangement?.witnesses.length ?? v?.engaged.length ?? 0;
    return (
      <main className="narrow">
        {header}
        <section className="card">
          {mine === 0 ? (
            <>
              <h2>Waiting for the others</h2>
              <p>
                You created this list. Nothing can be added until every member has joined and
                confirmed the start. Send them the invite link.
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
          {mine > 0 && (
            <button onClick={list.join} disabled={list.busy}>
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

  const items = v.state?.items ?? [];
  const pending = v.open?.candidates ?? [];
  const closed = v.is_final || v.state?.archived;
  const waiting = v.status.kind === "stalled" ? v.status.parties : [];

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    const t = text.trim();
    if (!t) return;
    setText("");
    void list.add(t);
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
          {items.map((it) => (
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
                title="remove"
                disabled={!!closed || list.busy}
                onClick={() => void list.remove(it.id)}
              >
                ×
              </button>
            </li>
          ))}
          {pending.map((k) => (
            <li key={k.hash} className="pending">
              <span className="dot" />
              <span>
                <Pending k={k} v={v} me={list.me} />
              </span>
            </li>
          ))}
          {items.length === 0 && pending.length === 0 && <li className="dim">Nothing on the list yet.</li>}
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
            const k = d.candidate.candidate;
            return (
              <div key={k.hash} className="ask">
                <p>
                  {partyLabel(v.parties, k.author, list.me)} proposes to <b>{k.kind}</b> this list
                  {k.kind === "close" ? " for good" : ""}. {k.votes} of {v.parties.length} have agreed.
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
            <button className="ghost" onClick={list.archive} disabled={list.busy}>
              Archive
            </button>
            <button className="ghost" onClick={list.close} disabled={list.busy}>
              Propose to close
            </button>
          </div>
        </section>
      )}

      {v.anomalies.length + v.suspects.length > 0 && (
        <section className="card warn">
          <h2>Something to look at</h2>
          <ul>
            {v.suspects.map((s) => (
              <li key={s.owner + s.path}>
                {short(s.owner)} holds a file whose bytes do not match its name: {s.path.split("/").pop()}
              </li>
            ))}
            {v.anomalies.map((a, i) => (
              <li key={i}>
                {a.kind}
                {a.against_pubky ? ` by ${partyLabel(v.parties, v.parties.indexOf(a.against_pubky), list.me)}` : ""}
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

function Members({
  parties,
  seated,
  me,
}: {
  parties: string[];
  seated: { pubky: string }[];
  me: string;
}) {
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

function Pending({ k, v, me }: { k: CandidateView; v: ChainView; me: string }) {
  const who = partyLabel(v.parties, k.author, me);
  const need = v.parties.length;
  return (
    <>
      {who}: <b>{k.kind}</b> · {k.votes}/{need} confirmed
    </>
  );
}
