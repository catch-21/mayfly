import type { ChainView, LoadedFile } from "../types";
import { fileKey } from "./Files";
import { Short } from "./Short";

/** Attributed anomalies and tampered-mirror suspects, each linking to the file that proves it. */
export function Anomalies({
  view,
  files,
  onSelect,
}: {
  view: ChainView;
  files: LoadedFile[];
  onSelect: (key: string) => void;
}) {
  const byH16 = new Map<string, LoadedFile[]>();
  for (const f of files) {
    if (!f.record) continue;
    const list = byH16.get(f.record.h16) ?? [];
    list.push(f);
    byH16.set(f.record.h16, list);
  }
  const total = view.anomalies.length + view.suspects.length;
  return (
    <section className="panel">
      <h2>Anomalies</h2>
      {total === 0 ? <p className="empty">none attributed</p> : null}
      <ul className="anomaly-list">
        {view.anomalies.map((a, i) => (
          <li key={`${a.kind}-${i}`}>
            <span className="kind">{a.kind}</span>
            {a.seq !== null ? <span className="dim"> at seq {a.seq}</span> : null}
            {a.against ? (
              <span>
                {" "}
                against <Short value={a.against} head={8} tail={4} />
                {a.against_pubky ? (
                  <>
                    {" "}
                    <span className="dim">(</span>
                    <Short value={a.against_pubky} head={10} tail={6} />
                    <span className="dim">)</span>
                  </>
                ) : (
                  <span className="dim"> (key not seated at the head)</span>
                )}
              </span>
            ) : (
              <span className="dim"> unattributed</span>
            )}
            <div className="dim">
              evidence:{" "}
              {a.evidence.length === 0 ? "none" : null}
              {a.evidence.map((h) => {
                const found = byH16.get(h);
                return (
                  <span key={h} style={{ marginRight: 8 }}>
                    {found?.length ? (
                      <a
                        href="#"
                        className="mono"
                        onClick={(e) => {
                          e.preventDefault();
                          onSelect(fileKey(found[0]));
                        }}
                      >
                        {h}
                      </a>
                    ) : (
                      <span className="mono" title="not among the listed files">
                        {h}
                      </span>
                    )}
                  </span>
                );
              })}
            </div>
          </li>
        ))}
        {view.suspects.map((s) => {
          const file = files.find((f) => f.folder.owner === s.owner && f.path === s.path);
          return (
            <li key={`${s.owner}${s.path}`}>
              <span className="kind" style={{ color: "var(--bad)" }}>
                TamperedMirror
              </span>
              <span className="dim"> bytes do not match name in </span>
              <Short value={s.owner} head={10} tail={6} />
              <div className="dim">
                {file ? (
                  <a
                    href="#"
                    className="mono"
                    onClick={(e) => {
                      e.preventDefault();
                      onSelect(fileKey(file));
                    }}
                  >
                    {s.path}
                  </a>
                ) : (
                  <span className="mono">{s.path}</span>
                )}
              </div>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
