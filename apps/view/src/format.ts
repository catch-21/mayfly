// Small display helpers: shortening identifiers and formatting times.

/** `abcdefgh…uvwxyz` for long identifiers; short ones unchanged. */
export function shorten(s: string, head = 8, tail = 6): string {
  if (s.length <= head + tail + 1) return s;
  return `${s.slice(0, head)}\u2026${s.slice(-tail)}`;
}

const timeFormat = new Intl.DateTimeFormat(undefined, {
  year: "numeric",
  month: "short",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
  hour12: false,
});

/** Unix milliseconds as local time. */
export function formatMillis(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return "\u2014";
  return timeFormat.format(new Date(ms));
}

/** Unix seconds as local time. */
export function formatSeconds(s: number): string {
  return formatMillis(s * 1000);
}

/** A clock time for "last updated". */
export function formatClock(d: Date): string {
  return d.toLocaleTimeString(undefined, { hour12: false });
}

/** Copy to the clipboard where the browser allows it; silent otherwise. */
export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

/** Pretty JSON, tolerant of values that cannot be serialised. */
export function prettyJson(v: unknown): string {
  try {
    return JSON.stringify(v, null, 2) ?? String(v);
  } catch {
    return String(v);
  }
}