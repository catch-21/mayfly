// What to highlight on a file row: the record's own checks (red), anomalies whose evidence is
// this record (amber), and the verifier's tampered-mirror suspects (red).

import type { AnomalyView, ChainView, LoadedFile } from "./types";

export interface Marks {
  /** Verification failures on this file: bad signature, hash mismatch, unreadable, tampered. */
  failures: string[];
  /** Anomaly kinds whose evidence includes this file's h16. */
  anomalies: AnomalyView[];
  /** True when the verifier listed this exact (owner, path) as a suspect. */
  suspect: boolean;
}

export function marksFor(file: LoadedFile, view: ChainView | null): Marks {
  const failures: string[] = [];
  const r = file.record;
  if (file.fetchError) failures.push(`could not fetch: ${file.fetchError}`);
  if (r) {
    if (r.error) failures.push(`not a record: ${r.error}`);
    if (r.signature_ok === false) failures.push("signature does not verify under the claimed key");
    if (r.hash_matches_etag === false) failures.push("bytes do not match the homeserver ETag");
    if (r.hash_matches_name === false) failures.push("bytes do not match the hash in the file name");
  }
  const suspect =
    view?.suspects.some((s) => s.owner === file.folder.owner && s.path === file.path) ?? false;
  if (suspect) failures.push("bytes do not match name \u2014 tampered mirror (\u00a77)");
  const anomalies =
    r && view ? view.anomalies.filter((a) => a.evidence.includes(r.h16)) : [];
  return { failures, anomalies, suspect };
}

export function rowClass(m: Marks): string {
  const c: string[] = [];
  if (m.failures.length) c.push("bad");
  if (m.anomalies.length) c.push("warn");
  return c.join(" ");
}
