import type { ChainView, Folder, LoadedFile } from "../types";
import { marksFor, rowClass } from "../marks";
import { Short } from "./Short";

function Chip({ label, value }: { label: string; value: boolean | null | undefined }) {
  const cls = value === true ? "ok" : value === false ? "bad" : "none";
  const text = value === true ? "ok" : value === false ? "FAIL" : "n/a";
  return (
    <span className={`chip ${cls}`} title={`${label}: ${text}`}>
      {label} {text}
    </span>
  );
}

function roleLabel(f: Folder): string {
  switch (f.role) {
    case "initiator":
      return "initiator";
    case "seat":
      return "seat";
    case "witness":
      return "witness";
  }
}

export function fileKey(f: LoadedFile): string {
  return `${f.folder.owner}${f.path}`;
}

/**
 * Every file found in every declared folder, grouped by folder, with the checks on each
 * record. Red rows failed a check on their own bytes; amber rows are evidence of an anomaly.
 */
export function Files({
  files,
  folders,
  view,
  selected,
  onSelect,
}: {
  files: LoadedFile[];
  folders: Folder[];
  view: ChainView | null;
  selected: string | null;
  onSelect: (key: string) => void;
}) {
  return (
    <section className="panel">
      <h2>Files</h2>
      <table className="files">
        <thead>
          <tr>
            <th>path</th>
            <th>typ</th>
            <th>checks</th>
          </tr>
        </thead>
        <tbody>
          {folders.map((folder) => {
            const inFolder = files.filter((f) => f.folder === folder);
            return [
              <tr key={`${folder.owner}${folder.prefix}`} className="folder-head">
                <td colSpan={3}>
                  <span className="badge">{roleLabel(folder)}</span>
                  <Short value={folder.owner} head={10} tail={6} />
                  <span className="mono"> {folder.prefix}</span>
                  {folder.error ? (
                    <span className="chip bad" style={{ marginLeft: 8 }}>
                      listing failed: {folder.error}
                    </span>
                  ) : null}
                  {!folder.error && inFolder.length === 0 ? (
                    <span className="dim" style={{ marginLeft: 8 }}>
                      (empty)
                    </span>
                  ) : null}
                </td>
              </tr>,
              ...inFolder.map((f) => {
                const key = fileKey(f);
                const m = marksFor(f, view);
                const r = f.record;
                return (
                  <tr
                    key={key}
                    className={`${rowClass(m)}${selected === key ? " selected" : ""}`}
                    onClick={() => onSelect(key)}
                  >
                    <td className="path mono">
                      {f.relative}
                      {m.failures.length || m.anomalies.length || m.suspect ? (
                        <div className="marks">
                          {m.failures.map((t) => (
                            <span key={t} className="chip bad">
                              {t}
                            </span>
                          ))}
                          {m.anomalies.map((a, i) => (
                            <span key={`${a.kind}-${i}`} className="chip warn">
                              anomaly: {a.kind}
                            </span>
                          ))}
                        </div>
                      ) : null}
                    </td>
                    <td className="mono">
                      {r ? (
                        r.typ ?? <span className="dim">{"\u2014"}</span>
                      ) : (
                        <span className="dim">not a .jws</span>
                      )}
                    </td>
                    <td>
                      {r ? (
                        <>
                          <Chip label="signature" value={r.signature_ok} />
                          <Chip label="bytes=etag" value={r.hash_matches_etag} />
                          <Chip label="bytes=name" value={r.hash_matches_name} />
                          {r.error ? <span className="chip bad">error</span> : null}
                        </>
                      ) : f.fetch_error ? (
                        <span className="chip bad">fetch failed</span>
                      ) : (
                        <span className="chip none">not decoded</span>
                      )}
                    </td>
                  </tr>
                );
              }),
            ];
          })}
        </tbody>
      </table>
      {files.length === 0 ? <p className="empty">no files listed</p> : null}
    </section>
  );
}
