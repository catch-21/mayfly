// Errors from the module are JS `Error`s whose `name` is the client variant. The module
// marks the ones that mean "wait for the chain to move" with `transient: true`; the loop
// retries those and never shows them.

export function isTransient(e: unknown): boolean {
  return e instanceof Error && (e as Error & { transient?: boolean }).transient === true;
}

/** The variant name, or `undefined` for anything that is not a named error. */
export function errorName(e: unknown): string | undefined {
  return e instanceof Error && e.name && e.name !== "Error" ? e.name : undefined;
}

/** One line for a person: `Name: message`, or the message alone. */
export function describeError(e: unknown): string {
  if (e instanceof Error) {
    const name = errorName(e);
    return name ? `${name}: ${e.message}` : e.message;
  }
  return String(e);
}

/** A user action the homeserver did not answer in time. */
export class TimeoutError extends Error {
  constructor(ms: number) {
    super(`no answer from the homeserver in ${Math.round(ms / 1000)} seconds; reload if this continues`);
    this.name = "Timeout";
  }
}

/**
 * Reject after `ms`. The underlying call is not cancelled (the SDK has no cancellation): the
 * client stays busy until the homeserver answers, and a reload starts afresh.
 */
export function withTimeout<T>(p: Promise<T>, ms: number): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const t = setTimeout(() => reject(new TimeoutError(ms)), ms);
    p.then(
      (v) => {
        clearTimeout(t);
        resolve(v);
      },
      (e) => {
        clearTimeout(t);
        reject(e);
      },
    );
  });
}
