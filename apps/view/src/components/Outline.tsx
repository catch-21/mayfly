import type { Loaded, LoadedFile } from "@synonymdev/mayfly-browser";

import { Json } from "./Json";
import { Short } from "./Short";

/** What can be read from the files alone, when this build cannot run the chain's rules. */
interface Sketch {
  rulesHash: string | null;
  quorum: number | null;
  parties: { pubky: string; role: string | null }[];
  witnesses: string[];
  links: { file: LoadedFile; seq: number; round: number | null; kind: string; author: string; body: unknown }[];
}

function objectOf(v: unknown): Record<string, unknown> | null {
  return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
}

function genesisBody(files: LoadedFile[]): Record<string, unknown> | null {
  const file = files.find((f) => f.relative.startsWith("links/00000000-") && f.relative.endsWith(".jws"));
  const payload = objectOf(file?.record?.payload);
  return objectOf(payload?.body);
}

/** Parties, named witnesses and links, taken from decoded records. Not a verification. */
function sketch(files: LoadedFile[]): Sketch {
  const body = genesisBody(files);
  const parties = Array.isArray(body?.parties)
    ? body.parties.flatMap((p) => {
        const o = objectOf(p);
        const pubky = o && typeof o.pubky === "string" ? o.pubky : null;
        if (!pubky) return [];
        return [{ pubky, role: typeof o?.role === "string" ? o.role : null }];
      })
    : [];
  const witnesses = Array.isArray(body?.witnesses)
    ? body.witnesses.flatMap((w) => {
        const pubky = objectOf(w)?.pubky;
        return typeof pubky === "string" ? [pubky] : [];
      })
    : [];
  const links = files
    .filter((f) => f.folder.role === "initiator" && f.relative.startsWith("links/") && f.record)
    .flatMap((file) => {
      const payload = objectOf(file.record?.payload);
      if (!payload || typeof payload.seq !== "number") return [];
      return [
        {
          file,
          seq: payload.seq,
          round: typeof payload.round === "number" ? payload.round : null,
          kind: typeof payload.kind === "string" ? payload.kind : "record",
          author: typeof payload.author === "string" ? payload.author : "",
          body: payload.body,
        },
      ];
    })
    .sort((a, b) => a.seq - b.seq);
  return {
    rulesHash: typeof body?.rules_hash === "string" ? body.rules_hash : null,
    quorum: typeof body?.confirm_quorum === "number" ? body.confirm_quorum : null,
    parties,
    witnesses,
    links,
  };
}

/**
 * The chain as the files read, without a verified view. Signature and hash checks are on
 * each file; nothing here says a link committed.
 */
export function Outline({ loaded }: { loaded: Loaded }) {
  const s = sketch(loaded.files);
  return (
    <>
      <section className="panel">
        <h2>Chain</h2>
        <p className="dim">
          These rules are not in this build, so this is the records as stored, not a verified chain.
        </p>
        <dl className="header-grid">
          <dt>Chain id</dt>
          <dd className="mono">{loaded.chain}</dd>
          <dt>Rules</dt>
          <dd className="mono">{loaded.rules ?? "\u2014"}</dd>
          <dt>Rules hash</dt>
          <dd className="mono">{s.rulesHash ?? "\u2014"}</dd>
          <dt>Quorum</dt>
          <dd>{s.quorum ?? "\u2014"}</dd>
          <dt>Parties</dt>
          <dd>
            <ul className="party-list">
              {s.parties.map((p, i) => (
                <li key={p.pubky}>
                  <span className="idx">{i}</span>
                  <Short value={p.pubky} head={10} tail={6} />
                  {p.role ? <span className="dim"> {p.role}</span> : null}
                </li>
              ))}
              {s.parties.length === 0 ? <li className="empty">not readable from genesis</li> : null}
            </ul>
          </dd>
          <dt>Witnesses named</dt>
          <dd>
            <ul className="party-list">
              {s.witnesses.map((w) => (
                <li key={w}>
                  <Short value={w} head={10} tail={6} />
                </li>
              ))}
              {s.witnesses.length === 0 ? <li className="empty">none named in genesis</li> : null}
            </ul>
          </dd>
        </dl>
      </section>
      <section className="panel">
        <h2>Links</h2>
        {s.links.length === 0 ? <p className="empty">No link records in the initiator's folder.</p> : null}
        {s.links.map((link) => {
          const r = link.file.record;
          const sig = r?.signature_ok;
          return (
            <article key={link.file.path} className="link-card">
              <div className="link-head">
                <span className="seq mono">#{link.seq}</span>
                <span className="kind">{link.kind}</span>
                {link.round !== null ? <span className="badge">round {link.round}</span> : null}
                <span className={`badge ${sig === true ? "ok" : sig === false ? "bad" : "none"}`}>
                  signature {sig === true ? "ok" : sig === false ? "failed" : "unread"}
                </span>
              </div>
              <div className="link-meta">
                <span>
                  author <Short value={link.author} head={10} tail={6} />
                </span>
                {r ? (
                  <span>
                    hash <Short value={r.hash} head={10} tail={6} />
                  </span>
                ) : null}
              </div>
              <Json value={link.body} />
            </article>
          );
        })}
      </section>
    </>
  );
}
