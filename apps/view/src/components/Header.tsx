import type { ChainView } from "../types";
import { formatSeconds } from "../format";
import { Short } from "./Short";

/** Chain id, rules, status, parties, seats and engaged witnesses at the head. */
export function Header({ view, rules }: { view: ChainView; rules: string | null }) {
  const statusClass =
    view.status.kind === "closed"
      ? "ok"
      : view.status.kind === "ongoing"
        ? "accent"
        : view.status.kind === "abandoned"
          ? "bad"
          : "warn";
  return (
    <section className="panel">
      <h2>Chain</h2>
      <dl className="header-grid">
        <dt>Chain id</dt>
        <dd className="mono">{view.chain}</dd>
        <dt>Rules</dt>
        <dd className="mono">{view.rules ?? rules ?? "\u2014"}</dd>
        <dt>Status</dt>
        <dd>
          <span className={`badge ${statusClass}`}>{view.status.kind}</span>
          {view.is_final ? (
            <span className="badge ok">final</span>
          ) : (
            <span className="badge warn">not final</span>
          )}
          <span>{view.status.summary}</span>
          {view.status.outcome ? <span className="dim"> (outcome: {view.status.outcome})</span> : null}
        </dd>
        <dt>Parties</dt>
        <dd>
          <ul className="party-list">
            {view.parties.map((p, i) => (
              <li key={i}>
                <span className="idx">{i}</span>
                <Short value={p} head={10} tail={6} />
                {view.status.parties.includes(i) ? (
                  <span className="badge warn" style={{ marginLeft: 8 }}>
                    {view.status.kind === "abandoned" ? "closed out" : "awaited"}
                  </span>
                ) : null}
              </li>
            ))}
            {view.parties.length === 0 ? <li className="empty">none known</li> : null}
          </ul>
        </dd>
        <dt>Seats</dt>
        <dd>
          <ul className="seat-list">
            {view.seats.map((s) => (
              <li key={`${s.pubky}/${s.kid}`}>
                <Short value={s.pubky} head={10} tail={6} />
                <span className="dim"> kid </span>
                <Short value={s.kid} head={8} tail={4} />
                <span className="dim"> app </span>
                <span className="mono">{s.client_id}</span>
                <span className="dim"> grant until {formatSeconds(s.grant_exp)}</span>
                <div className="paths mono">{s.paths.join("  ")}</div>
              </li>
            ))}
            {view.seats.length === 0 ? <li className="empty">no seats established</li> : null}
          </ul>
        </dd>
        <dt>Witnesses</dt>
        <dd>
          <ul className="seat-list">
            {view.engaged.map((w) => (
              <li key={`${w.pubky}/${w.kid}`}>
                <Short value={w.pubky} head={10} tail={6} />
                <span className="dim"> kid </span>
                <Short value={w.kid} head={8} tail={4} />
                <span className="dim">
                  {" "}
                  until {formatSeconds(w.until)}, polls every {w.poll_ms} ms
                </span>
                <div className="paths mono">{w.path}</div>
              </li>
            ))}
            {view.engaged.length === 0 ? <li className="empty">none engaged</li> : null}
          </ul>
        </dd>
      </dl>
    </section>
  );
}
