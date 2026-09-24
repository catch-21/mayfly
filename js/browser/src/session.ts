// One open chain for one member: a ChainClient, its latest view, and the loop that keeps it
// honest. Framework-free; a page subscribes to `state` and calls the actions.
//
// The shape is crates/client/tests/list.rs: the app proposes typed bodies and otherwise only
// calls act() — after every user action, on a timer, and when a member's homeserver reports a
// change. act() confirms valid proposals, re-proposes rules content after a dead round, and
// skips on its own; what comes back as a decision (a close, a recover) is put to the user
// here. What this file adds is everything the first web app had to learn the hard way:
// genesis terms before commit, a proposal held until the round is ready for it, decision
// cards that clear at the right moment, one event stream per other member and none for me,
// and a timeout that gives the form back.

import { isTransient, describeError, TimeoutError, withTimeout } from "./errors.js";
import { ChainClient, type Signer, type Store } from "./mayfly.js";
import type { RulesRef } from "./rules.js";
import type { ActionView, Arrangement, Body, CandidateView, ChainView, DecisionAction } from "./types.js";

/** Between timer-driven act() calls, in milliseconds. Events wake the loop sooner. */
export const DEFAULT_TICK_MS = 3_000;

/** How long a user action may take before the page gives up waiting and says so. */
export const DEFAULT_ACTION_TIMEOUT_MS = 20_000;

/** A source of "something changed on this homeserver folder" events. */
export interface Wake {
  /** Call `onEvent` whenever a file under `owner`'s `path` changes; returns the unsubscribe. */
  subscribe(owner: string, path: string, onEvent: () => void): () => void;
}

export interface Decision {
  action: DecisionAction;
  seenAt: number;
}

/** Where a member stands with a chain. */
export type Phase =
  /** Nothing read yet. */
  | "loading"
  /** Genesis names parties and I am not one of them. */
  | "stranger"
  /** I am named and have not signed genesis: show the terms, offer to join (§8.1). */
  | "invited"
  /** I have signed; genesis is not committed until everyone has. */
  | "waiting"
  /** Committed and ongoing. */
  | "open"
  /** Final: a close has committed and been sealed by its successor. */
  | "ended";

export interface ChainState<S = unknown, B extends Body = Body> {
  url: string;
  me: string;
  phase: Phase;
  view: ChainView<S> | undefined;
  /** Genesis terms, including before the chain commits (§8.1). */
  arrangement: Arrangement | undefined;
  /** The parties: from the committed genesis, or the arrangement before that. */
  parties: string[];
  /** My index among `parties`, or -1. */
  myIndex: number;
  /** The rules state at the head; `undefined` until genesis commits. */
  state: S | undefined;
  /** Candidates of the live round only; a dead round's are already voted on. */
  pending: CandidateView[];
  /** Questions for the person: a close, a recover, a "put it forward again?". */
  decisions: Decision[];
  /** Proposals held until the round is ready for them, in order. */
  held: B[];
  /** It is my round and nothing is held or carried: propose, or the loop passes for me. */
  myTurn: { round: number } | undefined;
  /** The head is unwitnessed and policy holds my vote (§11.2). */
  awaitingWitnesses: { have: number; of: number; want: number } | undefined;
  /** A user action is in flight. */
  busy: boolean;
  /** Something a person should read; cleared by the next success. */
  error: string | undefined;
  lastActions: ActionView[];
}

export interface ChainSessionOptions<B extends Body = Body> {
  rules: RulesRef;
  store: Store;
  signer: Signer;
  url: string;
  /** Live updates from the other members' homeservers. Without it, the timer alone drives. */
  wake?: Wake;
  tickMs?: number;
  actionTimeoutMs?: number;
  /**
   * In my round with nothing to propose or carry forward, pass so the others can proceed
   * (§6.4). Default true. Set false for rules where a round of mine is a move to make, and
   * answer `state.myTurn` yourself.
   */
  autoPass?: boolean;
  /** Replace the clock (tests). */
  clock?: () => number;
  /** Hold `propose` by default until the round accepts it. Default true. */
  holdProposals?: boolean;
  /** Type witness only. */
  readonly __body?: B;
}

type Listener<S, B extends Body> = (state: ChainState<S, B>) => void;

export class ChainSession<S = unknown, B extends Body = Body> {
  private client: ChainClient | undefined;
  private listeners = new Set<Listener<S, B>>();
  private timer: ReturnType<typeof setInterval> | undefined;
  private unsubscribe: (() => void)[] = [];
  private wakeKey = "";
  private inFlight = false;
  private alive = false;
  /** Every call into the client runs here, one after another (see `exclusive`). */
  private turn: Promise<unknown> = Promise.resolve();
  /**
   * Whether `state.error` came from the loop rather than from a user action. The loop clears
   * only its own errors on its next success; what a person was told about their own action
   * stays until their next action.
   */
  private loopError = false;
  private queue: B[] = [];
  private readonly tickMs: number;
  private readonly actionTimeoutMs: number;
  private readonly autoPass: boolean;
  private readonly holdProposals: boolean;
  private readonly wake: Wake | undefined;
  private readonly clock: (() => number) | undefined;
  private readonly rules: RulesRef;
  private readonly store: Store;
  private readonly signer: Signer;

  private _state: ChainState<S, B>;

  constructor(options: ChainSessionOptions<B>) {
    this.rules = options.rules;
    this.store = options.store;
    this.signer = options.signer;
    this.wake = options.wake;
    this.clock = options.clock;
    this.tickMs = options.tickMs ?? DEFAULT_TICK_MS;
    this.actionTimeoutMs = options.actionTimeoutMs ?? DEFAULT_ACTION_TIMEOUT_MS;
    this.autoPass = options.autoPass ?? true;
    this.holdProposals = options.holdProposals ?? true;
    this._state = {
      url: options.url,
      me: options.signer.pubky,
      phase: "loading",
      view: undefined,
      arrangement: undefined,
      parties: [],
      myIndex: -1,
      state: undefined,
      pending: [],
      decisions: [],
      held: [],
      myTurn: undefined,
      awaitingWitnesses: undefined,
      busy: false,
      error: undefined,
      lastActions: [],
    };
  }

  get state(): ChainState<S, B> {
    return this._state;
  }

  /** Listen for state changes; returns the unsubscribe. */
  subscribe(listener: Listener<S, B>): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  /** Open the client, sync, act, and keep going until `stop()`. */
  start(): void {
    if (this.alive) return;
    this.alive = true;
    void (async () => {
      try {
        const c = ChainClient.openUrl(this.rules, this.store, this.signer, this._state.url);
        if (this.clock) c.setClock(this.clock);
        if (!this.alive) return;
        this.client = c;
        const view = (await c.sync()) as ChainView<S>;
        if (!this.alive) return;
        this.publish(view);
        await this.act();
      } catch (e) {
        if (this.alive) {
          this.loopError = true;
          this.set({ error: describeError(e) });
        }
      }
    })();
    this.timer = setInterval(() => void this.act(), this.tickMs);
  }

  /** Close every stream and stop the timer. The client may still be inside a call. */
  stop(): void {
    this.alive = false;
    if (this.timer) clearInterval(this.timer);
    this.timer = undefined;
    for (const u of this.unsubscribe) u();
    this.unsubscribe = [];
    this.wakeKey = "";
  }

  // ── Actions ──────────────────────────────────────────────────────────────────────────────

  /** Sign genesis: consent to the parties, the rules and the fault model (§8.1). */
  join(): Promise<void> {
    return this.run((c) => c.join());
  }

  /**
   * Propose rules content. Held (default) means: if the round is not ready for it — my
   * proposal was just refused, or the round is another member's — it is kept and tried on
   * every tick until it goes out, and shows in `state.held`. Not held means it is tried once
   * and a transient refusal is an error.
   */
  propose(body: B, options: { hold?: boolean } = {}): Promise<void> {
    const hold = options.hold ?? this.holdProposals;
    if (!hold) return this.run((c) => c.proposeBody(body));
    this.queue.push(body);
    this.set({ held: [...this.queue] });
    return this.run(async (c) => {
      // Only the head of the queue can go out this round; a second proposal would be a
      // second vote (§6.4).
      if (this.queue[0] !== body) return;
      try {
        await c.proposeBody(body);
        this.dequeue(body);
      } catch (e) {
        if (isTransient(e)) return;
        this.dequeue(body);
        throw e;
      }
    });
  }

  /** Take back a held proposal that has not gone out. */
  withdraw(body: B): void {
    this.dequeue(body);
  }

  confirm(hash: string): Promise<void> {
    return this.answer(hash, (c) => c.confirm(hash));
  }

  reject(hash: string): Promise<void> {
    return this.answer(hash, (c) => c.reject(hash));
  }

  /** After a dead round, as the designated proposer: put `earlier` forward again (§6.4). */
  repropose(earlier: string): Promise<void> {
    return this.answer(earlier, (c) => c.repropose(earlier));
  }

  /** After a dead round, as the designated proposer: let it go and pass the round (§6.4). */
  pass(): Promise<void> {
    this.set({ decisions: this._state.decisions.filter((d) => !d.action.repropose), myTurn: undefined });
    return this.run((c) => c.pass());
  }

  /** An ordinary close; the others answer through their decision cards (§6.8). */
  proposeClose(reason: "agreed" | "finished"): Promise<void> {
    return this.run((c) => c.proposeClose(reason));
  }

  /** Close on silent parties, by index (§6.8). */
  proposeAbandoned(subjects: number[]): Promise<void> {
    return this.run((c) => c.proposeAbandoned(Uint32Array.from(subjects)));
  }

  confirmAbandoned(hash: string): Promise<void> {
    return this.answer(hash, (c) => c.confirmAbandoned(hash));
  }

  /** Sync and act now. */
  refresh(): Promise<void> {
    return this.act();
  }

  // ── The loop ─────────────────────────────────────────────────────────────────────────────

  /**
   * Run `f` after every earlier call into the client has finished. The wasm client refuses a
   * second call while one is in flight (`Busy`); a user's click must wait its turn, not be
   * dropped because an event woke the loop a moment earlier. A timed-out call gives up its
   * place in the queue; the client stays busy until the homeserver answers, and later calls
   * see `Busy`, which the loop treats as "try again next tick".
   */
  private exclusive<T>(f: () => Promise<T>, timeoutMs?: number): Promise<T> {
    const next = this.turn.then(() => (timeoutMs === undefined ? f() : withTimeout(f(), timeoutMs)));
    this.turn = next.catch(() => undefined);
    return next;
  }

  /** act(), then publish the view; one at a time, never throwing. */
  private async act(): Promise<void> {
    if (!this.client || this.inFlight || !this.alive) return;
    this.inFlight = true;
    try {
      await this.exclusive(() => this.step());
    } catch (e) {
      if (this.alive && !isTransient(e)) {
        this.loopError = true;
        this.set({ error: describeError(e) });
      }
    } finally {
      this.inFlight = false;
    }
  }

  /** One turn of the loop, inside `exclusive`. */
  private async step(): Promise<void> {
    const c = this.client;
    if (!c || !this.alive) return;
    {
      const actions = (await c.act()) as ActionView[];
      if (!this.alive) return;
      // A proposal that could not go out when it was made is tried again on every tick until
      // it is proposed. In my own round it also answers a "put it forward again, or let it
      // go" question about my dead proposal: a new proposal is the third answer (§6.4), so
      // that card is not shown. A transient error means "not my turn yet".
      let proposed = false;
      const head = this.queue[0];
      if (head !== undefined) {
        try {
          await c.proposeBody(head);
          this.dequeue(head);
          proposed = true;
        } catch (e) {
          if (!isTransient(e)) {
            this.dequeue(head);
            throw e;
          }
        }
      }
      let decisions = this._state.decisions;
      let myTurn: ChainState["myTurn"];
      if (!proposed) {
        const asks = actions.filter((a): a is DecisionAction => a.kind === "decision");
        if (asks.length) {
          const known = new Set(decisions.map((d) => d.action.candidate.hash));
          const fresh = asks.filter((a) => !known.has(a.candidate.hash)).map((action) => ({ action, seenAt: Date.now() }));
          if (fresh.length) decisions = [...decisions, ...fresh];
        }
        const turn = actions.find((a): a is Extract<ActionView, { kind: "my_turn" }> => a.kind === "my_turn");
        if (turn) {
          if (this.autoPass) {
            // A round of mine with nothing to carry forward: pass, so the others can proceed.
            try {
              await c.pass();
            } catch (e) {
              if (!isTransient(e)) throw e;
            }
          } else {
            myTurn = { round: turn.round };
          }
        }
      }
      const awaiting = actions.find((a): a is Extract<ActionView, { kind: "awaiting_witnesses" }> => a.kind === "awaiting_witnesses");
      const view = c.view() as ChainView<S> | undefined;
      this.publish(view, {
        decisions,
        myTurn,
        awaitingWitnesses: awaiting ? { have: awaiting.have, of: awaiting.of, want: awaiting.want } : undefined,
        lastActions: actions,
        error: this.loopError ? undefined : this._state.error,
      });
      if (this.loopError) this.loopError = false;
    }
  }

  /** A user action: run it with a timeout, then act() so confirmations and mirrors follow. */
  private async run(f: (c: ChainClient) => Promise<unknown>): Promise<void> {
    const c = this.client;
    if (!c) {
      this.set({ error: "The chain is still opening; try again in a moment." });
      return;
    }
    this.set({ busy: true, error: undefined });
    this.loopError = false;
    let timedOut = false;
    try {
      await this.exclusive(() => f(c), this.actionTimeoutMs);
    } catch (e) {
      timedOut = e instanceof TimeoutError;
      this.set({ error: isTransient(e) ? "The chain is settling a change; try again in a moment." : describeError(e) });
    } finally {
      this.set({ busy: false });
    }
    // After a timeout the client is still inside the stalled call; act() would only report
    // Busy. The timer keeps trying.
    if (!timedOut) await this.act();
  }

  /** Answer a decision card: the card goes at once; act() then shows what follows. */
  private answer(hash: string, f: (c: ChainClient) => Promise<unknown>): Promise<void> {
    this.set({ decisions: this._state.decisions.filter((d) => d.action.candidate.hash !== hash) });
    return this.run(f);
  }

  private dequeue(body: B): void {
    const i = this.queue.indexOf(body);
    if (i >= 0) this.queue.splice(i, 1);
    this.set({ held: [...this.queue] });
  }

  /** Derive everything a page reads from the view, and open streams for the seats. */
  private publish(view: ChainView<S> | undefined, patch: Partial<ChainState<S, B>> = {}): void {
    const c = this.client;
    const arrangement = (c?.arrangement() as Arrangement | undefined) ?? this._state.arrangement;
    const parties = view && view.parties.length > 0 ? view.parties : (arrangement?.parties ?? []);
    const me = this._state.me;
    const myIndex = parties.indexOf(me);
    const seated = view?.seats.some((s) => s.pubky === me) ?? false;
    const committed = !!view && view.parties.length > 0;
    const open = view?.open ?? null;
    // A decision is live while its candidate is still up for a vote and I have not voted in
    // the round it asks about. `open.round` is the dead round once it has died; a
    // re-proposal is a question about the next round, so a vote in the dead round must not
    // hide it (§6.4).
    const decisions = (patch.decisions ?? this._state.decisions).filter((d) => {
      if (!open) return false;
      const k = d.action.candidate;
      const voted = open.voters.some(([round, who]) => round === d.action.round && who.includes(myIndex));
      if (voted) return false;
      const live = open.candidates.some((x) => x.hash === k.hash);
      return live && (d.action.repropose || k.round === open.round);
    });
    // A dead round's candidates have already been voted on. Showing them as pending would
    // make a refused close look still open and hide that the next round has started.
    const pending = open && !open.dead ? open.candidates.filter((k) => k.round === open.round) : [];
    let phase: Phase;
    if (parties.length === 0) phase = "loading";
    else if (myIndex < 0) phase = "stranger";
    else if (!seated) phase = "invited";
    else if (!committed) phase = "waiting";
    else if (view.is_final) phase = "ended";
    else phase = "open";
    this.set({
      ...patch,
      view,
      arrangement,
      parties,
      myIndex,
      state: (view?.state as S | null | undefined) ?? undefined,
      pending,
      decisions,
      phase,
    });
    this.watch(view);
  }

  /**
   * Live: one event stream per other member's homeserver folder; any event wakes act(). My
   * own writes call act() directly, so my folder needs no stream. Each stream holds an HTTP
   * connection open, and a browser allows only a few per host, so streams are opened
   * sparingly and always closed when the seat set changes or the session stops.
   */
  private watch(view: ChainView<S> | undefined): void {
    if (!this.wake || !view) return;
    const seats = view.seats.filter((s) => s.pubky !== this._state.me);
    const key = seats.map((s) => `${s.pubky}:${s.paths.join(",")}`).join("|");
    if (key === this.wakeKey) return;
    for (const u of this.unsubscribe) u();
    this.unsubscribe = [];
    this.wakeKey = key;
    for (const seat of seats) {
      for (const path of seat.paths) {
        this.unsubscribe.push(this.wake.subscribe(seat.pubky, `${path}chains/${view.chain}/`, () => void this.act()));
      }
    }
  }

  private set(patch: Partial<ChainState<S, B>>): void {
    this._state = { ...this._state, ...patch };
    for (const l of this.listeners) l(this._state);
  }
}
