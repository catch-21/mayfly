import type { ChainView, LoadedFile } from "../types";
import { marksFor } from "../marks";
import { Json } from "./Json";
import { Short } from "./Short";

/** One record in full: decoded payload, claimed key, hashes, the checks, and the raw JWS. */
export function RecordPanel({
  file,
  view,
  onClose,
}: {
  file: LoadedFile;
  view: ChainView | null;
  onClose: () => void;
}) {
  const r = file.record;
  const m = marksFor(file, view);
  return (
    <div className="record-panel">
      <h3>
        <span className="mono">{file.relative}</span>
        <span className="badge">{file.folder.role}</span>
        <Short value={file.folder.owner} head={10} tail={6} />
        <span className="spacer" />
        <button onClick={onClose}>close</button>
      </h3>
      {m.failures.map((t) => (
        <div key={t} className="flag bad">
          {t}
        </div>
      ))}
      {m.anomalies.map((a, i) => (
        <div key={`${a.kind}-${i}`} className="flag warn">
          evidence of anomaly <strong>{a.kind}</strong>
          {a.seq !== null ? ` at seq ${a.seq}` : ""}
          {a.against ? (
            <>
              {" "}
              against <Short value={a.against_pubky ?? a.against} head={10} tail={6} />
            </>
          ) : null}
        </div>
      ))}
      {!r ? (
        <p className="empty">{file.fetch_error ?? "this file is not a .jws record"}</p>
      ) : (
        <>
          <dl className="header-grid">
            <dt>typ</dt>
            <dd className="mono">{r.typ ?? "\u2014"}</dd>
            <dt>claimed key</dt>
            <dd className="mono">{r.claimed_key ?? "\u2014"}</dd>
            <dt>hash</dt>
            <dd className="mono">
              {r.hash} <span className="dim">h16</span> {r.h16}
            </dd>
            <dt>ETag</dt>
            <dd className="mono">{file.content_hash ?? <span className="dim">not reported</span>}</dd>
            <dt>checks</dt>
            <dd>
              signature {String(r.signature_ok ?? "n/a")}, bytes=etag {String(r.hash_matches_etag ?? "n/a")},
              bytes=name {String(r.hash_matches_name ?? "n/a")}
              {r.error ? `, error: ${r.error}` : ""}
            </dd>
          </dl>
          <p className="dim" style={{ margin: "10px 0 0" }}>
            payload (embedded confirmations, receipts and Grants unpacked)
          </p>
          <Json value={r.payload} />
          <details>
            <summary>raw JWS ({r.raw.length} bytes)</summary>
            <pre className="raw mono">{r.raw}</pre>
          </details>
        </>
      )}
    </div>
  );
}
