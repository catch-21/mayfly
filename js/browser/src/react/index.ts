// React bindings: one hook per thing a page shows. Each wraps a framework-free object from
// the package root, so a page written without React reads the same state.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { GrantAuthFlow, Session } from "@synonymdev/pubky";

import type { MayflyApp, Party } from "../auth.js";
import { createChain, myChains } from "../chains.js";
import { describeError } from "../errors.js";
import { ChainReader, type ChainReaderOptions, type ReaderState } from "../reader.js";
import type { RulesRef } from "../rules.js";
import { ChainSession, type ChainSessionOptions, type ChainState } from "../session.js";
import type { Body, ChainSpec, MyChain } from "../types.js";
import { pubkyWake } from "../wake.js";

/**
 * The signed-in session: restored from the browser store on first render, `null` when there
 * is none, `undefined` while looking. `signIn` remembers; `signOut` forgets.
 */
export function useSession(app: MayflyApp): {
  session: Session | null | undefined;
  party: Party | undefined;
  error: string | undefined;
  signIn(session: Session): Promise<void>;
  signOut(): Promise<void>;
} {
  const [session, setSession] = useState<Session | null>();
  const [party, setParty] = useState<Party>();
  const [error, setError] = useState<string>();

  useEffect(() => {
    let cancelled = false;
    Promise.all([app.ready(), app.restore()])
      .then(([, s]) => {
        if (!cancelled) setSession(s ?? null);
      })
      .catch((e) => {
        if (!cancelled) setError(describeError(e));
      });
    return () => {
      cancelled = true;
    };
  }, [app]);

  useEffect(() => {
    if (!session) {
      setParty(undefined);
      return;
    }
    let cancelled = false;
    app
      .party(session)
      .then((p) => {
        if (!cancelled) setParty(p);
      })
      .catch((e) => {
        if (!cancelled) setError(describeError(e));
      });
    return () => {
      cancelled = true;
    };
  }, [app, session]);

  const signIn = useCallback(
    async (s: Session) => {
      setSession(s);
      await app.remember(s);
    },
    [app],
  );
  const signOut = useCallback(async () => {
    await app.forget(session ?? undefined);
    setSession(null);
  }, [app, session]);

  return { session, party, error, signIn, signOut };
}

/**
 * A Ring sign-in: starts the flow on mount, renders the QR, and calls `onSession` when the
 * user's device approves.
 */
export function useRingSignIn(
  app: MayflyApp,
  onSession: (s: Session) => void,
): { flow: GrantAuthFlow | undefined; qr: string | undefined; error: string | undefined } {
  const [flow, setFlow] = useState<GrantAuthFlow>();
  const [qr, setQr] = useState<string>();
  const [error, setError] = useState<string>();
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const f = await app.startRingSignIn();
        if (cancelled) return;
        setFlow(f);
        setQr(await app.qrDataUrl(f));
        const session = await f.awaitApproval();
        if (!cancelled) onSession(session);
      } catch (e) {
        if (!cancelled) setError(describeError(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [app, onSession]);
  return { flow, qr, error };
}

/** The chains this member is on, from their own homeserver; `reload` after creating one. */
export function useMyChains(app: MayflyApp, party: Party | undefined): {
  chains: MyChain[] | undefined;
  error: string | undefined;
  reload(): void;
} {
  const [chains, setChains] = useState<MyChain[]>();
  const [error, setError] = useState<string>();
  const [tick, setTick] = useState(0);
  useEffect(() => {
    if (!party) return;
    let cancelled = false;
    myChains(party.store, app.folder)
      .then((c) => {
        if (!cancelled) setChains(c);
      })
      .catch((e) => {
        if (!cancelled) setError(describeError(e));
      });
    return () => {
      cancelled = true;
    };
  }, [app, party, tick]);
  return { chains, error, reload: () => setTick((t) => t + 1) };
}

/** Create a chain as `party`; resolves to the invite URL. */
export function useCreateChain(party: Party | undefined, rules: RulesRef): {
  create(spec: Omit<ChainSpec, "parties"> & { parties: string[] }): Promise<string>;
  busy: boolean;
  error: string | undefined;
} {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const create = useCallback(
    async (spec: ChainSpec) => {
      if (!party) throw new Error("not signed in");
      setBusy(true);
      setError(undefined);
      try {
        return await createChain(rules, party.store, party.signer, spec);
      } catch (e) {
        setError(describeError(e));
        throw e;
      } finally {
        setBusy(false);
      }
    },
    [party, rules],
  );
  return { create, busy, error };
}

export type SessionHandle<S, B extends Body> = ChainState<S, B> &
  Pick<
    ChainSession<S, B>,
    | "join"
    | "propose"
    | "withdraw"
    | "confirm"
    | "reject"
    | "repropose"
    | "pass"
    | "proposeClose"
    | "proposeAbandoned"
    | "confirmAbandoned"
    | "refresh"
  >;

/**
 * One open chain for one member: state plus actions. Opens a `ChainSession` per (party, url),
 * with live updates from the other members' homeservers (`options.wake` to supply your own),
 * and stops it when the page leaves.
 */
export function useChainSession<S = unknown, B extends Body = Body>(
  app: MayflyApp,
  party: Party,
  url: string,
  rules: RulesRef,
  options: Omit<ChainSessionOptions<B>, "rules" | "store" | "signer" | "url"> = {},
): SessionHandle<S, B> {
  const optionsRef = useRef(options);
  optionsRef.current = options;
  const session = useMemo(
    () =>
      new ChainSession<S, B>({
        ...optionsRef.current,
        rules,
        store: party.store,
        signer: party.signer,
        url,
        wake: optionsRef.current.wake ?? pubkyWake(app.pubky),
      }),
    [app, party, url, rules],
  );
  const [state, setState] = useState<ChainState<S, B>>(session.state);
  useEffect(() => {
    setState(session.state);
    const off = session.subscribe(setState);
    session.start();
    return () => {
      off();
      session.stop();
    };
  }, [session]);
  return useMemo(
    () => ({
      ...state,
      join: () => session.join(),
      propose: (body: B, o?: { hold?: boolean }) => session.propose(body, o),
      withdraw: (body: B) => session.withdraw(body),
      confirm: (h: string) => session.confirm(h),
      reject: (h: string) => session.reject(h),
      repropose: (h: string) => session.repropose(h),
      pass: () => session.pass(),
      proposeClose: (r: "agreed" | "finished") => session.proposeClose(r),
      proposeAbandoned: (s: number[]) => session.proposeAbandoned(s),
      confirmAbandoned: (h: string) => session.confirmAbandoned(h),
      refresh: () => session.refresh(),
    }),
    [state, session],
  );
}

/** Follow a chain as a bystander; polls until it is final. */
export function useChainReader<S = unknown>(
  options: ChainReaderOptions | undefined,
): ReaderState<S> & { refresh(): Promise<void> } {
  const reader = useMemo(
    () => (options ? new ChainReader<S>(options) : undefined),
    // The store and registry are long-lived; the URL and poll interval are what change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [options?.store, options?.registry, options?.url, options?.pollMs],
  );
  const idle: ReaderState<S> = { url: options?.url ?? "", loaded: undefined, error: undefined, busy: false, final: false };
  const [state, setState] = useState<ReaderState<S>>(reader?.state ?? idle);
  useEffect(() => {
    if (!reader) {
      setState(idle);
      return;
    }
    setState(reader.state);
    const off = reader.subscribe(setState);
    reader.start();
    return () => {
      off();
      reader.stop();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reader]);
  return { ...state, refresh: () => reader?.refresh() ?? Promise.resolve() };
}
