// One open list: a ChainClient, its latest view, and the loop that keeps it honest.
//
// The shape is crates/client/tests/list.rs (and the wasm test beside it): the app proposes
// typed bodies and otherwise only calls act() — after every user action, on a timer, and
// when a member's homeserver reports a change. act() confirms valid proposals, re-proposes
// after a dead round, passes and skips on its own; what comes back as a decision (a close)
// is put to the user here.

import { useCallback, useEffect, useRef, useState } from "react";
import { PublicKey, type Session } from "@synonymdev/pubky";

import {
  ChainClient,
  RULES,
  party,
  type ActionView,
  type Arrangement,
  type ChainView,
  type Signer,
  type Store,
} from "./mayfly";
import { pubky } from "./pubky";

/** Between timer-driven act() calls, in milliseconds. Events wake the loop sooner. */
const TICK_MS = 3_000;

export interface Decision {
  candidate: ActionView & { kind: "decision" };
  seenAt: number;
}

export interface ListHandle {
  view: ChainView | undefined;
  /** Genesis terms, including before the chain commits. */
  arrangement: Arrangement | undefined;
  me: string;
  myIndex: number | undefined;
  /** Seated: my genesis confirmation is in; before that, the app shows the consent screen. */
  seated: boolean;
  /** A party named in genesis, seated or not. */
  member: boolean;
  inviteUrl: string;
  busy: boolean;
  error: string | undefined;
  decisions: Decision[];
  lastActions: ActionView[];
  join(): Promise<void>;
  add(text: string): Promise<void>;
  tick(id: string, on: boolean): Promise<void>;
  remove(id: string): Promise<void>;
  archive(): Promise<void>;
  close(): Promise<void>;
  confirm(hash: string): Promise<void>;
  reject(hash: string): Promise<void>;
  refresh(): Promise<void>;
}

/** A short random id for a new item; ids are per list, never shown. */
function itemId(): string {
  const bytes = new Uint8Array(6);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

function message(e: unknown): string {
  if (e instanceof Error) return e.name && e.name !== "Error" ? `${e.name}: ${e.message}` : e.message;
  return String(e);
}

/** Errors that mean "wait for the chain to move", not "something is wrong". */
function transient(e: unknown): boolean {
  return e instanceof Error && ["AlreadyVoted", "RoundDead", "NotDesignated", "Busy", "AwaitingWitnesses"].includes(e.name);
}

export function useList(session: Session, url: string): ListHandle {
  const [view, setView] = useState<ChainView>();
  const [arrangement, setArrangement] = useState<Arrangement>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [decisions, setDecisions] = useState<Decision[]>([]);
  const [lastActions, setLastActions] = useState<ActionView[]>([]);
  const client = useRef<ChainClient>();
  const pieces = useRef<{ store: Store; signer: Signer }>();
  const inFlight = useRef(false);
  const alive = useRef(true);
  const me = session.info.publicKey.z32();

  /** act(), then publish the view; one at a time, never throwing. */
  const act = useCallback(async () => {
    const c = client.current;
    if (!c || inFlight.current) return;
    inFlight.current = true;
    try {
      const actions = (await c.act()) as ActionView[];
      if (!alive.current) return;
      setLastActions(actions);
      const asks = actions.filter((a): a is ActionView & { kind: "decision" } => a.kind === "decision");
      if (asks.length) {
        setDecisions((prev) => {
          const known = new Set(prev.map((d) => d.candidate.candidate.hash));
          const fresh = asks.filter((a) => !known.has(a.candidate.hash)).map((a) => ({ candidate: a, seenAt: Date.now() }));
          return fresh.length ? [...prev, ...fresh] : prev;
        });
      }
      // A round of mine with nothing to carry forward: pass, so the others can proceed (§6.4).
      if (actions.some((a) => a.kind === "my_turn")) {
        try {
          await c.pass();
        } catch (e) {
          if (!transient(e)) throw e;
        }
      }
      const v = c.view() as ChainView | undefined;
      setView(v);
      setArrangement(c.arrangement() as Arrangement | undefined);
      setDecisions((prev) => prev.filter((d) => v?.open?.candidates.some((k) => k.hash === d.candidate.candidate.hash)));
      setError(undefined);
    } catch (e) {
      if (alive.current && !transient(e)) setError(message(e));
    } finally {
      inFlight.current = false;
    }
  }, []);

  // Open the client once per (session, url).
  useEffect(() => {
    alive.current = true;
    setView(undefined);
    setError(undefined);
    setDecisions([]);
    client.current = undefined;
    (async () => {
      try {
        const p = await party(session);
        if (!alive.current) return;
        pieces.current = p;
        const c = ChainClient.openUrl(RULES, p.store, p.signer, url);
        client.current = c;
        const v = (await c.sync()) as ChainView;
        if (!alive.current) return;
        setView(v);
        setArrangement(c.arrangement() as Arrangement | undefined);
        await act();
      } catch (e) {
        if (alive.current) setError(message(e));
      }
    })();
    return () => {
      alive.current = false;
    };
  }, [session, url, act]);

  // The timer.
  useEffect(() => {
    const t = setInterval(() => void act(), TICK_MS);
    return () => clearInterval(t);
  }, [act]);

  // Live: one event stream per seated member's homeserver folder; any event wakes act().
  const seatKey = view?.seats.map((s) => `${s.pubky}:${s.paths.join(",")}`).join("|") ?? "";
  useEffect(() => {
    if (!view || !seatKey) return;
    const chain = view.chain;
    const readers: ReadableStreamDefaultReader<unknown>[] = [];
    let stopped = false;
    for (const seat of view.seats) {
      for (const path of seat.paths) {
        (async () => {
          try {
            const stream = await pubky
              .eventStreamForUser(PublicKey.from(seat.pubky), null)
              .live()
              .path(`${path}chains/${chain}/`)
              .subscribe();
            const reader = stream.getReader();
            readers.push(reader);
            for (;;) {
              const { done } = await reader.read();
              if (done || stopped) break;
              void act();
            }
          } catch {
            // No stream (an old homeserver, a network blip): the timer still drives act().
          }
        })();
      }
    }
    return () => {
      stopped = true;
      for (const r of readers) void r.cancel().catch(() => undefined);
    };
  }, [seatKey, view?.chain, act]);

  /** A user action: run it, then act() so confirmations and mirrors follow at once. */
  const run = useCallback(
    async (f: (c: ChainClient) => Promise<unknown>) => {
      const c = client.current;
      if (!c) return;
      setBusy(true);
      try {
        await f(c);
        setError(undefined);
      } catch (e) {
        setError(transient(e) ? "The list is settling a change; try again in a moment." : message(e));
      } finally {
        setBusy(false);
      }
      await act();
    },
    [act],
  );

  const myIndex = view?.parties.indexOf(me);
  return {
    view,
    arrangement,
    me,
    myIndex: myIndex === undefined || myIndex < 0 ? undefined : myIndex,
    seated: view?.seats.some((s) => s.pubky === me) ?? false,
    member: view?.parties.includes(me) ?? false,
    inviteUrl: url,
    busy,
    error,
    decisions,
    lastActions,
    join: () => run((c) => c.join()),
    add: (text) => run((c) => c.proposeBody({ kind: "add", id: itemId(), text })),
    tick: (id, on) => run((c) => c.proposeBody({ kind: on ? "tick" : "untick", id })),
    remove: (id) => run((c) => c.proposeBody({ kind: "remove", id })),
    archive: () => run((c) => c.proposeBody({ kind: "archive" })),
    close: () => run((c) => c.proposeClose("agreed")),
    confirm: (hash) => run((c) => c.confirm(hash)),
    reject: (hash) => run((c) => c.reject(hash)),
    refresh: act,
  };
}

/**
 * Create a list: genesis names every member (me first) and, optionally, a watchman. Returns
 * the invite URL.
 */
export async function createList(session: Session, members: string[], witnesses: string[]): Promise<string> {
  const p = await party(session);
  const me = session.info.publicKey.z32();
  const parties = [me, ...members.filter((m) => m !== me)];
  const c = await ChainClient.create(RULES, p.store, p.signer, {
    parties,
    apps: parties.map(() => p.signer.clientId),
    witnesses,
  });
  return c.inviteUrl();
}

/** My lists, from the `index/active/` and `index/finished/` markers in my folder (§7). */
export async function myLists(session: Session): Promise<{ url: string; finished: boolean }[]> {
  const p = await party(session);
  const me = session.info.publicKey.z32();
  const folder = `/pub/${p.signer.clientId}/mayfly/`;
  const out: { url: string; finished: boolean }[] = [];
  for (const [sub, finished] of [
    ["index/active/", false],
    ["index/finished/", true],
  ] as const) {
    const listed = await p.store.list(me, folder + sub);
    for (const entry of listed) {
      const bytes = await p.store.get(me, entry.path);
      if (bytes) out.push({ url: new TextDecoder().decode(bytes).trim(), finished });
    }
  }
  return out;
}
