/** `abcd…wxyz` for a pubky or hash. */
export function short(s: string, n = 6): string {
  return s.length <= 2 * n + 1 ? s : `${s.slice(0, n)}…${s.slice(-n)}`;
}

/** A member's label: "you", or a shortened pubky. */
export function partyLabel(parties: string[], index: number, me: string): string {
  const p = parties[index];
  if (!p) return `party ${index}`;
  return p === me ? "you" : short(p);
}

export async function copy(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}
