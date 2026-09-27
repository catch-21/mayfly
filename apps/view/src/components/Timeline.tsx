import type { ChainView, LinkView, OpenSeqView, WitnessTime } from "../types";
import { formatDuration, formatMillis } from "../format";
import { Json } from "./Json";
import { Short } from "./Short";

function witnessTime(label: string, time: WitnessTime, asDuration: boolean): string {
  if (time.ms !== null && time.ms !== undefined) {
    return `${label} ${asDuration ? formatDuration(time.ms) : formatMillis(time.ms)}`;
  }
  if (time.split.length) {
    const parts = time.split.map(
      (r) => `${r.witness.slice(0, 8)} ${asDuration ? formatDuration(r.ms) : formatMillis(r.ms)}`,
    );
    return `${label} split ${parts.join(", ")}`;
  }
  return `${label} not yet timed`;
}

function LinkCard({ link, parties }: { link: LinkView; parties: string[] }) {
  const [m, k] = link.witnessed;
  return (
    <article className={`link-card${link.is_final ? "" : " provisional"}`} id={`seq-${link.seq}`}>
      <div className="link-head">
        <span className="seq mono">#{link.seq}</span>
        <span className="kind">{link.kind}</span>
        <span className="badge">round {link.round}</span>
        {link.is_final ? (
          <span className="badge ok">final</span>
        ) : (
          <span className="badge warn">provisional (head)</span>
        )}
        <span className={`badge ${k === 0 ? "none" : m === k ? "ok" : "warn"}`}>
          witnessed {m}/{k}
        </span>
        <span className="dim mono">h16 {link.h16}</span>
      </div>
      <div className="link-meta">
        <span>
          author {link.author}{" "}
          <Short value={link.author_pubky || parties[link.author] || ""} head={10} tail={6} />
        </span>
        <span>author's clock {formatMillis(link.ts)}</span>
        <span>{witnessTime("seen", link.observed_at, false)}</span>
        <span>{witnessTime("confirmed", link.confirmed_at, false)}</span>
        <span>{witnessTime("think", link.think, true)}</span>
        <span>{witnessTime("confirm delay", link.respond, true)}</span>
        <span>
          confirmers{" "}
          {link.confirmers.length ? link.confirmers.map((c) => `#${c}`).join(", ") : "none recorded"}
        </span>
        <span>
          hash <Short value={link.hash} head={10} tail={6} />
        </span>
      </div>
      <Json value={link.body} />
    </article>
  );
}

function OpenSeq({ open }: { open: OpenSeqView }) {
  return (
    <div className="open-seq">
      <h3>
        Open seq #{open.seq}
        <span className="badge warn" style={{ marginLeft: 10 }}>
          round {open.round}
        </span>
        {open.dead ? <span className="badge bad">round dead</span> : <span className="badge">round live</span>}
      </h3>
      {open.candidates.length ? (
        <table className="plain">
          <thead>
            <tr>
              <th>candidate</th>
              <th>author</th>
              <th>kind</th>
              <th>round</th>
              <th>votes</th>
            </tr>
          </thead>
          <tbody>
            {open.candidates.map((c) => (
              <tr key={c.hash}>
                <td className="mono">
                  {c.h16} <span className="dim">(</span>
                  <Short value={c.hash} head={8} tail={4} />
                  <span className="dim">)</span>
                </td>
                <td>#{c.author}</td>
                <td>{c.kind}</td>
                <td>{c.round}</td>
                <td>{c.votes}</td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <p className="empty">no valid candidate yet</p>
      )}
      {open.voters.length ? (
        <p className="dim">
          voters by round:{" "}
          {open.voters.map(([round, ps]) => `r${round}: ${ps.map((p) => `#${p}`).join(", ") || "none"}`).join("; ")}
        </p>
      ) : null}
    </div>
  );
}

/** Committed links in seq order, then the open seq when there is one. */
export function Timeline({ view }: { view: ChainView }) {
  return (
    <section className="panel">
      <h2>Timeline</h2>
      <div className="timeline">
        {view.committed.map((l) => (
          <LinkCard key={l.hash} link={l} parties={view.parties} />
        ))}
        {view.committed.length === 0 ? <p className="empty">nothing committed yet</p> : null}
        {view.open ? <OpenSeq open={view.open} /> : null}
      </div>
    </section>
  );
}
