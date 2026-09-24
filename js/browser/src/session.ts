// One open chain for one member, kept honest by the module's client. This file owns what a
// browser has and the module does not: a timer, event streams from the other members'
// homeservers, a timeout that gives the form back, and what a person is told. Everything
// about rounds, votes, held proposals and where the member stands is the client's
// (`act()`, `session()`), and comes back as data.

import { describeError, isTransient, TimeoutError, withTimeout } from "./errors.js";
import { ChainClient, type Signer, type Store } from "./mayfly.js";
import type { RulesRef } from "./rules.js";
import type { ActionView, Arrangement, Body, CandidateView, ChainView, DecisionAction, Phase, SessionView } from "./types.js";

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

export interface ChainState<S = unknown, B extends Body = Body> {
  url: string;
  me: string;
  /** Where I stand (§8.1). */
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
  /** Candidates of the live round only. */
  pending: CandidateView[];
  /** Questions for the person, as the client asked them on its last step. */
  decisions: Decision[];
  /** Proposals held until a round takes them, in order. */
  held: B[];
  /** With `autoPass: false`: my round and nothing to carry; propose, or `pass()`. */
  myTurn: { round: number } | undefined;
  /** The head is unwitnessed and policy holds my vote (§11.2). */
  awaitingWitnesses: { have: number; of: number; want: number } | undefined;
  /** A user action is in flight. */
  busy: boolean;
  /** Something a person should read; cleared by their next action. */
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

/** The key of one question: which candidate, and whether it is "confirm?" or "carry forward?". */
function question(hash: string, repropose: boolean): string {
  return `${repropose ? "carry" : "vote"}:${hash}`;
}

export class ChainSession<S = unknown, B extends Body = Body> {
  private client: ChainClient | undefined;
  private listeners = new Set<Listener<S, B>>();
  private timer: ReturnType<typeof setInterval> | undefined;
  private unsubscribe: (() => void)[] = [];
  private wakeKey = "";
  private ticking = false;
  private alive = false;
  /**
   * Questions answered since the last step, hidden until the client stops asking them. Keyed
   * by the question, not the candidate: refusing a close and then being asked whether to
   * put that same close forward again are two questions about one hash.
   */
  private answered = new Set<string>();
  /** Whether `state.error` is the loop's, and so cleared by its next success. */
  private loopError = false;
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
        c.setPolicy({ autoPass: this.autoPass });
        if (!this.alive) return;
        this.client = c;
        await c.sync();
        if (!this.alive) return;
        this.publish();
        await this.act();
      } catch (e) {
        if (this.alive) this.fail(e, true);
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
   * proposal was just refused, or the round is another member's — the client keeps it and
   * proposes it on a later `act()`, and it shows in `state.held`. Not held means it is tried
   * once and a refusal of any kind is an error.
   */
  propose(body: B, options: { hold?: boolean } = {}): Promise<void> {
    if (!(options.hold ?? this.holdProposals)) return this.run((c) => c.proposeBody(body));
    return this.run((c) => c.hold(body));
  }

  /** Take back a held proposal that has not gone out. */
  withdraw(body: B): Promise<void> {
    return this.run(async (c) => {
      const held = (c.session() as SessionView<B> | undefined)?.held ?? [];
      const i = held.findIndex((h) => JSON.stringify(h) === JSON.stringify(body));
      if (i >= 0) await c.withdraw(i);
    });
  }

  confirm(hash: string): Promise<void> {
    return this.answer(question(hash, false), (c) => c.confirm(hash));
  }

  reject(hash: string): Promise<void> {
    return this.answer(question(hash, false), (c) => c.reject(hash));
  }

  /** After a dead round, as the designated proposer: put `earlier` forward again (§6.4). */
  repropose(earlier: string): Promise<void> {
    return this.answer(question(earlier, true), (c) => c.repropose(earlier));
  }

  /** After a dead round, as the designated proposer: let it go and pass the round (§6.4). */
  pass(): Promise<void> {
    for (const d of this._state.decisions) if (d.action.repropose) this.answered.add(question(d.action.candidate.hash, true));
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
    return this.answer(question(hash, false), (c) => c.confirmAbandoned(hash));
  }

  /** Sync and act now. */
  refresh(): Promise<void> {
    return this.act();
  }

  // ── The loop ─────────────────────────────────────────────────────────────────────────────

  /** One `act()`, then publish; ticks that arrive while one is running are coalesced. */
  private async act(): Promise<void> {
    const c = this.client;
    if (!c || this.ticking || !this.alive) return;
    this.ticking = true;
    try {
      const actions = (await c.act()) as ActionView[];
      if (!this.alive) return;
      const asked = new Set(
        actions.filter((a): a is DecisionAction => a.kind === "decision").map((a) => question(a.candidate.hash, a.repropose)),
      );
      for (const q of [...this.answered]) if (!asked.has(q)) this.answered.delete(q);
      const refused = actions.find((a): a is Extract<ActionView, { kind: "held_refused" }> => a.kind === "held_refused");
      const turn = actions.find((a): a is Extract<ActionView, { kind: "my_turn" }> => a.kind === "my_turn");
      const awaiting = actions.find((a): a is Extract<ActionView, { kind: "awaiting_witnesses" }> => a.kind === "awaiting_witnesses");
      this.publish({
        lastActions: actions,
        myTurn: turn ? { round: turn.round } : undefined,
        awaitingWitnesses: awaiting ? { have: awaiting.have, of: awaiting.of, want: awaiting.want } : undefined,
        // A held proposal the rules refused for good is the person's to hear about.
        error: refused ? `${refused.body.kind} was refused: ${refused.reason}` : this.loopError ? undefined : this._state.error,
      });
      this.loopError = false;
    } catch (e) {
      if (this.alive && !isTransient(e)) this.fail(e, true);
    } finally {
      this.ticking = false;
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
      // The client queues calls, so this waits for a tick in flight rather than racing it.
      await withTimeout(f(c), this.actionTimeoutMs);
    } catch (e) {
      timedOut = e instanceof TimeoutError;
      this.set({ error: isTransient(e) ? "The chain is settling a change; try again in a moment." : describeError(e) });
    } finally {
      this.set({ busy: false });
    }
    // After a timeout the client is still inside the stalled call; the timer keeps trying.
    if (!timedOut) await this.act();
  }

  /** Answer a decision card: the card goes at once; act() then shows what follows. */
  private answer(q: string, f: (c: ChainClient) => Promise<unknown>): Promise<void> {
    this.answered.add(q);
    this.set({ decisions: this._state.decisions.filter((d) => question(d.action.candidate.hash, d.action.repropose) !== q) });
    return this.run(f);
  }

  private fail(e: unknown, fromLoop: boolean): void {
    this.loopError = fromLoop;
    this.set({ error: describeError(e) });
  }

  /** Read the client's snapshot into the page's state, and open streams for the seats. */
  private publish(patch: Partial<ChainState<S, B>> = {}): void {
    const c = this.client;
    if (!c) return;
    const view = c.view() as ChainView<S> | undefined;
    const s = c.session() as SessionView<B>;
    const actions = patch.lastActions ?? this._state.lastActions;
    const decisions = actions
      .filter((a): a is DecisionAction => a.kind === "decision" && !this.answered.has(question(a.candidate.hash, a.repropose)))
      .map((action) => {
        const seen = this._state.decisions.find(
          (d) => d.action.candidate.hash === action.candidate.hash && d.action.repropose === action.repropose,
        );
        return { action, seenAt: seen?.seenAt ?? Date.now() };
      });
    this.set({
      ...patch,
      view,
      arrangement: c.arrangement() as Arrangement | undefined,
      phase: s.phase,
      parties: s.parties,
      myIndex: s.my_index ?? -1,
      state: (c.state() as S | undefined) ?? undefined,
      pending: s.pending,
      held: s.held,
      decisions,
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
