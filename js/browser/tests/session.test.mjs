// The browser loop, run in Node over a memory store: two members' ChainSessions and a
// ChainReader, with a shared clock and no timers. These are the behaviours the list app got
// wrong first (SKILL.md, "Rules of the loop"), pinned so the next app cannot.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { KeyedSigner, designatedProposer } from "@synonymdev/mayfly";

import {
  ChainReader,
  ChainSession,
  RulesRegistry,
  createChain,
  loadMayfly,
  myChains,
  parsePubkys,
  shippedRules,
} from "../dist/core.js";
import { Shared, memoryStore } from "./memory-store.mjs";

await loadMayfly(readFileSync(new URL("../node_modules/@synonymdev/mayfly/pkg/mayfly_bg.wasm", import.meta.url)));

const APP = "list.example";
const FOLDER = `/pub/${APP}/mayfly/`;
const THINK_MS = 60_000;

let now = 1_757_779_812_000;
const clock = () => now;

/** Wait until `pred(state)` holds, driving `refresh()` on every session in between. */
async function until(sessions, pred, what) {
  for (let i = 0; i < 40; i++) {
    for (const s of sessions) await s.refresh();
    if (pred()) return;
    now += THINK_MS + 1;
  }
  assert.fail(`timed out waiting for ${what}: ${JSON.stringify(sessions.map((s) => s.state.phase))}`);
}

function party(shared) {
  const signer = new KeyedSigner(APP);
  return { signer, store: memoryStore(signer.pubky, shared), me: signer.pubky };
}

/** A session that never ticks on its own: the tests drive it with `refresh()`. */
function open(p, url, shared, extra = {}) {
  const s = new ChainSession({
    rules: "list/1",
    store: p.store,
    signer: p.signer,
    url,
    wake: shared.wake(),
    tickMs: 1e9,
    actionTimeoutMs: 5_000,
    clock,
    ...extra,
  });
  s.start();
  return s;
}

const texts = (s) => (s.state.state?.items ?? []).map((i) => i.text);

async function twoMembers() {
  const shared = new Shared();
  const a = party(shared);
  const b = party(shared);
  const url = await createChain("list/1", a.store, a.signer, {
    parties: [a.me, b.me],
    options: { time_control: { think_ms: THINK_MS, respond_ms: THINK_MS } },
  });
  const sa = open(a, url, shared);
  const sb = open(b, url, shared);
  return { shared, a, b, url, sa, sb, stop: () => (sa.stop(), sb.stop()) };
}

test("the shipped rules and a pubky list are known before anything is written", () => {
  assert.deepEqual(shippedRules(), ["list/1"]);
  const registry = new RulesRegistry(shippedRules());
  assert.equal(registry.resolve("list/1"), "list/1");
  assert.equal(registry.resolve("chess/1"), undefined);
  const [me, other, third] = [0, 1, 2].map(() => new KeyedSigner(APP).pubky);
  assert.deepEqual(parsePubkys(`${me}\n${other}, ${third}`, { exclude: me }), [other, third]);
  assert.throws(() => parsePubkys(`${other} ${other}`), /listed more than once/);
  assert.throws(() => parsePubkys("not-a-pubky"), /is not a pubky/);
});

test("a member is invited, then waiting, then open; a stranger stays a stranger", async () => {
  const { shared, a, b, url, sa, sb, stop } = await twoMembers();
  try {
    await until([sa, sb], () => sa.state.phase === "waiting" && sb.state.phase === "invited", "terms read");
    // Before commit the terms come from the arrangement, not the empty committed genesis.
    assert.deepEqual(sb.state.parties, [a.me, b.me]);
    assert.equal(sb.state.arrangement?.confirm_quorum, 2);
    assert.equal(sb.state.view?.parties.length, 0);
    assert.equal(sb.state.state, undefined);
    assert.equal(sb.state.myIndex, 1);

    const stranger = party(shared);
    const sx = open(stranger, url, shared);
    await until([sx], () => sx.state.phase === "stranger", "stranger");
    sx.stop();

    await sb.join();
    await until([sa, sb], () => sa.state.phase === "open" && sb.state.phase === "open", "genesis committed");
    assert.deepEqual(texts(sa), []);
    assert.deepEqual(await myChains(a.store, FOLDER), [{ url, finished: false }]);
  } finally {
    stop();
  }
});

test("an add is proposed by one member and confirmed by the other inside the loop", async () => {
  const { sa, sb, stop } = await twoMembers();
  try {
    await until([sa, sb], () => sb.state.phase === "invited", "invited");
    await sb.join();
    await until([sa, sb], () => sa.state.phase === "open", "open");
    await sa.propose({ kind: "add", id: "milk", text: "Milk" });
    await until([sa, sb], () => texts(sb).includes("Milk"), "Milk confirmed");
    assert.deepEqual(texts(sa), ["Milk"]);
    assert.equal(sa.state.pending.length, 0);
    assert.equal(sa.state.held.length, 0);
    assert.equal(sa.state.error, undefined);
  } finally {
    stop();
  }
});

test("a refused agreed close is consent withheld: the proposer gets the card, an add is the third answer", async () => {
  const { sa, sb, stop } = await twoMembers();
  try {
    await until([sa, sb], () => sb.state.phase === "invited", "invited");
    await sb.join();
    await until([sa, sb], () => sa.state.phase === "open", "open");
    await sa.propose({ kind: "add", id: "milk", text: "Milk" });
    await until([sa, sb], () => texts(sb).includes("Milk"), "Milk");

    await sa.proposeClose("agreed");
    await until([sa, sb], () => sb.state.decisions.length === 1, "B is asked");
    const ask = sb.state.decisions[0].action;
    assert.equal(ask.candidate.kind, "close");
    assert.equal(ask.repropose, false);
    // The pending row shows the close in the live round.
    assert.equal(sb.state.pending.length, 1);

    await sb.reject(ask.candidate.hash);
    assert.ok(
      sb.state.decisions.every((d) => d.action.repropose),
      "the agree/refuse card goes at once; only a 'put it forward again?' card may follow",
    );
    await until([sa, sb], () => sa.state.view?.open?.dead === true || sa.state.decisions.length > 0, "round dead");
    // The dead round's candidates are no longer pending, and no one is accused.
    assert.equal(sa.state.pending.length, 0);
    assert.ok(!sa.state.view.anomalies.some((x) => x.kind === "Obstruction"), JSON.stringify(sa.state.view.anomalies));

    // The next round has one designated proposer, who is asked whether to put the close
    // forward again (§6.4). If that is B, B lets it go; the round then rotates to A. If it is
    // A, the add typed now is A's third answer and the card is not shown. Either way the add
    // is held until it can go out, and no error is shown for the wait.
    const carried = [sa, sb].find((s) => s.state.decisions.some((d) => d.action.repropose));
    if (carried === sb) await sb.pass();
    await sa.propose({ kind: "add", id: "pears", text: "Pears" });
    assert.equal(sa.state.error, undefined, "a wait for the round is not an error");
    await until([sa, sb], () => texts(sb).includes("Pears"), "Pears confirmed after the refused close");
    assert.deepEqual(texts(sa), ["Milk", "Pears"]);
    assert.equal(sa.state.decisions.length, 0);
    assert.equal(sa.state.held.length, 0);
    assert.equal(sa.state.phase, "open");
    assert.ok(!sa.state.view.anomalies.some((x) => x.kind === "Obstruction"));
  } finally {
    stop();
  }
});

test("competing proposals converge with no user involvement, and the loser can propose again", async () => {
  const { sa, sb, stop } = await twoMembers();
  try {
    await until([sa, sb], () => sb.state.phase === "invited", "invited");
    await sb.join();
    await until([sa, sb], () => sa.state.phase === "open", "open");
    // Both propose at once: the round dies; the next round's designated proposer carries
    // the lowest-hash link forward inside act() (§6.4). Exactly one commits.
    await Promise.all([
      sa.propose({ kind: "add", id: "eggs", text: "Eggs" }),
      sb.propose({ kind: "add", id: "bread", text: "Bread" }),
    ]);
    await until([sa, sb], () => texts(sa).length === 1 && texts(sb).length === 1, "one of the two committed");
    const [winner] = texts(sa);
    assert.ok(["Eggs", "Bread"].includes(winner));
    assert.deepEqual(texts(sb), [winner]);
    for (const s of [sa, sb]) {
      assert.equal(s.state.held.length, 0);
      assert.equal(s.state.decisions.length, 0);
      assert.equal(s.state.error, undefined);
    }
    // The other item was not chosen; its author adds it again and it goes through.
    const loser = winner === "Eggs" ? sb : sa;
    const text = winner === "Eggs" ? "Bread" : "Eggs";
    await loser.propose({ kind: "add", id: `${text}-2`, text });
    await until([sa, sb], () => texts(sa).length === 2 && texts(sb).length === 2, "the loser's item committed");
    assert.deepEqual(texts(sa), [winner, text]);
  } finally {
    stop();
  }
});

test("an oversized body is refused before it is written and the list is unchanged", async () => {
  const { sa, sb, stop } = await twoMembers();
  try {
    await until([sa, sb], () => sb.state.phase === "invited", "invited");
    await sb.join();
    await until([sa, sb], () => sa.state.phase === "open", "open");
    await sa.propose({ kind: "add", id: "kept", text: "Kept" });
    await until([sa, sb], () => texts(sb).includes("Kept"), "Kept");
    await sa.propose({ kind: "add", id: "big", text: "Z".repeat(70_000) });
    assert.match(sa.state.error ?? "", /^add was refused: record is \d+ bytes; this chain allows 65536/);
    assert.equal(sa.state.held.length, 0, "not held: the refusal is final");
    await until([sa, sb], () => sa.state.view?.committed.length === 2, "settled");
    assert.deepEqual(texts(sb), ["Kept"]);
  } finally {
    stop();
  }
});

test("closing moves the chain to finished in the home index and the reader stops polling", async () => {
  const { shared, a, b, url, sa, sb, stop } = await twoMembers();
  try {
    await until([sa, sb], () => sb.state.phase === "invited", "invited");
    await sb.join();
    await until([sa, sb], () => sa.state.phase === "open", "open");

    const registry = new RulesRegistry(shippedRules());
    const reader = new ChainReader({ store: memoryStore("", shared), registry, url, pollMs: 1e9 });
    await reader.refresh();
    assert.equal(reader.state.loaded.view.is_final, false);
    assert.equal(reader.state.final, false);
    assert.equal(reader.state.loaded.rules, "list/1");
    assert.ok(reader.state.loaded.files.some((f) => /links\/00000000-/.test(f.relative)));

    await sa.propose({ kind: "archive" });
    await until([sa, sb], () => sa.state.state?.archived === true, "archived");
    await sb.proposeClose("finished");
    await until([sa, sb], () => sa.state.decisions.length === 1, "A is asked");
    await sa.confirm(sa.state.decisions[0].action.candidate.hash);
    await until([sa, sb], () => sa.state.phase === "ended" && sb.state.phase === "ended", "final");
    assert.equal(sa.state.view.status.kind, "closed");
    assert.deepEqual(await myChains(a.store, FOLDER), [{ url, finished: true }]);
    assert.deepEqual(await myChains(b.store, FOLDER), [{ url, finished: true }]);

    await reader.refresh();
    assert.equal(reader.state.final, true);
    assert.equal(reader.state.loaded.view.committed.length, sa.state.view.committed.length);
    assert.equal(reader.state.loaded.folders.filter((f) => f.role === "seat").length, 1);
  } finally {
    stop();
  }
});

test("a reader without the chain's rules still lists and decodes the files", async () => {
  const shared = new Shared();
  const a = party(shared);
  const b = party(shared);
  const tally = {
    id: "tally/1",
    referenceHash: "tally-test",
    init: (g) => ({ n: 0, parties: g.parties.map((p) => p.pubky) }),
    mayAppend: () => true,
    apply: (s, l) => ({ ...s, n: s.n + l.body.n }),
    close: () => ({ summary: "done" }),
  };
  const url = await createChain(tally, a.store, a.signer, { parties: [a.me, b.me] });
  const without = new RulesRegistry(shippedRules());
  const blind = new ChainReader({ store: memoryStore("", shared), registry: without, url, pollMs: 1e9 });
  await blind.refresh();
  assert.equal(blind.state.loaded.view, null);
  assert.equal(blind.state.loaded.rules, "tally/1");
  assert.equal(blind.state.loaded.rules_known, false);
  assert.match(blind.state.loaded.view_error, /no rules available for "tally\/1"/);
  assert.ok(blind.state.loaded.files.length > 0);

  const withRules = new RulesRegistry(shippedRules(), [tally]);
  const sighted = new ChainReader({ store: memoryStore("", shared), registry: withRules, url, pollMs: 1e9 });
  await sighted.refresh();
  assert.ok(sighted.state.loaded.view, sighted.state.loaded.view_error ?? "");
  assert.equal(sighted.state.loaded.rules_known, true);
});

// ── What the browser package owns, as opposed to the client underneath it ────────────────────

/**
 * An open two-member list, both seated, with `extra` session options applied to both. With
 * `slowB`, B has no wake and moves only when a test calls `refresh()` on it, so A's
 * proposals stay pending as long as the test wants.
 */
async function openList(extra = {}, { slowB = false } = {}) {
  const shared = new Shared();
  const a = party(shared);
  const b = party(shared);
  const url = await createChain("list/1", a.store, a.signer, {
    parties: [a.me, b.me],
    options: { time_control: { think_ms: THINK_MS, respond_ms: THINK_MS } },
  });
  const sa = open(a, url, shared, extra);
  const sb = open(b, url, shared, slowB ? { ...extra, wake: undefined } : extra);
  await until([sa, sb], () => sb.state.phase === "invited", "invited");
  await sb.join();
  await until([sa, sb], () => sa.state.phase === "open" && sb.state.phase === "open", "open");
  return { shared, a, b, url, sa, sb, stop: () => (sa.stop(), sb.stop()) };
}

/** Like `until`, without moving the clock: nobody is skipped for silence. */
async function untilNow(sessions, pred, what) {
  for (let i = 0; i < 40; i++) {
    for (const s of sessions) await s.refresh();
    if (pred()) return;
  }
  assert.fail(
    `timed out waiting for ${what}: ${JSON.stringify(
      sessions.map((s) => ({ phase: s.state.phase, open: s.state.view?.open, last: s.state.lastActions, error: s.state.error })),
    )}`,
  );
}

/** Whose round `round` at the open seq is, as the sessions see it. */
function designatedSession(sa, sb, round) {
  const v = sa.state.view;
  const who = designatedProposer(v.chain, v.open.seq, round, 2);
  return sa.state.parties[who] === sa.state.me ? sa : sb;
}

test("a held proposal shows as held and can be withdrawn before it goes out", async () => {
  const { sa, sb, stop } = await openList({}, { slowB: true });
  try {
    // A proposes so A's vote in round 0 is spent; B has not confirmed, so a second body
    // from A is held.
    await sa.propose({ kind: "add", id: "one", text: "One" });
    await sa.propose({ kind: "add", id: "two", text: "Two" });
    assert.deepEqual(
      sa.state.held.map((h) => h.text).filter(Boolean),
      ["Two"],
      "the second add waits for a round",
    );
    assert.equal(sa.state.error, undefined, "waiting is not an error");
    await sa.withdraw({ kind: "add", id: "two", text: "Two" });
    assert.deepEqual(sa.state.held, []);
    await until([sa, sb], () => texts(sb).includes("One"), "One confirmed");
    assert.deepEqual(texts(sa), ["One"], "the withdrawn add never went out");
  } finally {
    stop();
  }
});

test("an unheld proposal is tried once: refused now is an error, not a wait", async () => {
  const { sa, sb, stop } = await openList({}, { slowB: true });
  try {
    await sa.propose({ kind: "add", id: "one", text: "One" }, { hold: false });
    // A's vote in round 0 is spent; a second unheld proposal cannot go out now.
    await sa.propose({ kind: "add", id: "two", text: "Two" }, { hold: false });
    assert.deepEqual(sa.state.held, [], "not held");
    assert.match(sa.state.error ?? "", /settling a change/);
    await until([sa, sb], () => texts(sb).includes("One"), "One confirmed");
    assert.deepEqual(texts(sa), ["One"]);
    // A rules refusal on an unheld proposal is named.
    await sa.propose({ kind: "tick", id: "nothing" }, { hold: false });
    assert.match(sa.state.error ?? "", /^Rules: /);
  } finally {
    stop();
  }
});

test("letting a refused close go hands the question to the next proposer; a new proposal ends it", async () => {
  // `autoPass: false` here so that nothing is passed on anyone's behalf: every move below is
  // the test's own, and the cards are what the client asked.
  const { sa, sb, stop } = await openList({ autoPass: false });
  try {
    await sa.propose({ kind: "add", id: "milk", text: "Milk" });
    await until([sa, sb], () => texts(sb).includes("Milk"), "Milk");
    await sa.proposeClose("agreed");
    await until([sa, sb], () => sb.state.decisions.length === 1, "B is asked");
    await sb.reject(sb.state.decisions[0].action.candidate.hash);
    await until([sa, sb], () => sa.state.view?.open?.dead === true || sa.state.decisions.length > 0, "round 0 dead");

    // Round 1: the designated proposer is asked whether to carry the close, and lets it go.
    const first = designatedSession(sa, sb, 1);
    const second = first === sa ? sb : sa;
    await untilNow([sa, sb], () => first.state.decisions.some((d) => d.action.repropose), "carry-forward card");
    await first.pass();
    assert.equal(first.state.decisions.length, 0, "pass() clears the card at once");
    for (let i = 0; i < 3; i++) await first.refresh();
    assert.equal(first.state.decisions.length, 0, "and it does not come back to the one who let it go");

    // Round 2 is the other member's. The close still has its author's vote at this seq, so
    // the client carries it forward (§6.4) and asks them too; nobody is passed for. Nothing
    // moves until a person answers.
    await untilNow([sa, sb], () => second.state.decisions.some((d) => d.action.repropose), "the question moves on");
    assert.equal(second.state.decisions[0].action.round, 2);
    assert.equal(second.state.myTurn, undefined, "a carried close is a card, not an empty turn");
    const before = second.state.view.committed.length;
    for (let i = 0; i < 3; i++) await second.refresh();
    assert.equal(second.state.view.committed.length, before, "nothing moved on its own");
    assert.ok(!second.state.lastActions.some((a) => a.kind === "passed"), "nobody passed for them");

    // A new proposal is the third answer: it goes out in their round, the card goes, and the
    // list moves on with no obstruction on either side.
    await second.propose({ kind: "add", id: "pears", text: "Pears" });
    await untilNow([sa, sb], () => texts(sa).includes("Pears") && texts(sb).includes("Pears"), "Pears confirmed");
    assert.equal(second.state.decisions.length, 0);
    assert.equal(first.state.decisions.length, 0);
    assert.ok(!sa.state.view.anomalies.some((x) => x.kind === "Obstruction"), JSON.stringify(sa.state.view.anomalies));
    assert.equal(sa.state.error, undefined);
    assert.equal(sb.state.error, undefined);
  } finally {
    stop();
  }
});

test("the wake watches the other members' folders, never mine, and lets go on stop()", async () => {
  const shared = new Shared();
  const a = party(shared);
  const b = party(shared);
  const url = await createChain("list/1", a.store, a.signer, { parties: [a.me, b.me] });
  const subs = [];
  const wake = {
    subscribe(owner, path, onEvent) {
      const s = { owner, path, onEvent, open: true };
      subs.push(s);
      return () => {
        s.open = false;
      };
    },
  };
  const sa = open(a, url, shared, { wake });
  const sb = open(b, url, shared, { wake });
  try {
    await until([sa, sb], () => sb.state.phase === "invited", "invited");
    await sb.join();
    await until([sa, sb], () => sa.state.phase === "open" && sb.state.phase === "open", "open");
    const mine = subs.filter((s) => s.open && s.owner === a.me);
    const theirs = subs.filter((s) => s.open && s.owner === b.me);
    // A watches only B; B watches only A. A folder is watched once, at the chain's path.
    assert.equal(mine.length, 1, "B watches A once");
    assert.equal(theirs.length, 1, "A watches B once");
    assert.ok(theirs[0].path.endsWith(`chains/${sa.state.view.chain}/`), theirs[0].path);

    // An event on B's folder is enough to make A act: B adds, A is woken, and A confirms
    // without any timer or refresh() on A.
    const chain = sa.state.view.chain;
    await sb.propose({ kind: "add", id: "milk", text: "Milk" });
    for (let i = 0; i < 20 && !texts(sa).includes("Milk"); i++) {
      for (const s of subs.filter((s) => s.open && s.owner === b.me)) s.onEvent();
      await new Promise((r) => setTimeout(r, 25));
      await sb.refresh();
    }
    assert.ok(texts(sa).includes("Milk"), "A confirmed from the wake alone");
    assert.equal(sa.state.view.chain, chain);

    sa.stop();
    assert.ok(subs.filter((s) => s.owner === b.me).every((s) => !s.open), "A let go of B's stream");
    assert.ok(subs.filter((s) => s.owner === a.me).some((s) => s.open), "B still watches A");
  } finally {
    sa.stop();
    sb.stop();
  }
});

test("the timer drives the loop on its own", async () => {
  const shared = new Shared();
  const a = party(shared);
  const b = party(shared);
  const url = await createChain("list/1", a.store, a.signer, { parties: [a.me, b.me] });
  // No wake: the only thing that can move A is its timer.
  const sa = open(a, url, shared, { wake: undefined, tickMs: 20 });
  const sb = open(b, url, shared, { wake: undefined, tickMs: 1e9 });
  try {
    await until([sb], () => sb.state.phase === "invited", "invited");
    await sb.join();
    await until([sb], () => sb.state.phase === "open", "B open");
    await sb.propose({ kind: "add", id: "milk", text: "Milk" });
    const start = Date.now();
    while (!texts(sa).includes("Milk") && Date.now() - start < 5000) {
      await new Promise((r) => setTimeout(r, 30));
      await sb.refresh();
    }
    assert.ok(texts(sa).includes("Milk"), `A confirmed on its timer: ${JSON.stringify(sa.state.phase)}`);
  } finally {
    sa.stop();
    sb.stop();
  }
});

test("a user action with no answer gives the form back and says so", async () => {
  const shared = new Shared();
  const a = party(shared);
  const b = party(shared);
  const url = await createChain("list/1", a.store, a.signer, { parties: [a.me, b.me] });
  // A store whose writes hang once asked to.
  let hang = false;
  const slow = { ...a.store, put: (path, bytes) => (hang ? new Promise(() => {}) : a.store.put(path, bytes)) };
  const sa = open({ ...a, store: slow }, url, shared, { actionTimeoutMs: 150 });
  const sb = open(b, url, shared);
  try {
    await until([sa, sb], () => sb.state.phase === "invited", "invited");
    await sb.join();
    await until([sa, sb], () => sa.state.phase === "open", "open");
    hang = true;
    let sawBusy = false;
    const off = sa.subscribe((s) => {
      if (s.busy) sawBusy = true;
    });
    await sa.propose({ kind: "add", id: "milk", text: "Milk" }, { hold: false });
    off();
    assert.ok(sawBusy, "the form was busy while waiting");
    assert.equal(sa.state.busy, false, "the form is released");
    assert.match(sa.state.error ?? "", /no answer from the homeserver in 0 seconds; reload/);
  } finally {
    sa.stop();
    sb.stop();
  }
});

test("an error about my action stays until my next action; a loop error clears on the next success", async () => {
  const shared = new Shared();
  const a = party(shared);
  const b = party(shared);
  const url = await createChain("list/1", a.store, a.signer, { parties: [a.me, b.me] });
  let failListsOnce = false;
  const flaky = {
    ...a.store,
    list: (owner, prefix) => {
      if (failListsOnce) {
        failListsOnce = false;
        return Promise.reject(new Error("homeserver away"));
      }
      return a.store.list(owner, prefix);
    },
  };
  const sa = open({ ...a, store: flaky }, url, shared);
  const sb = open(b, url, shared);
  try {
    await until([sa, sb], () => sb.state.phase === "invited", "invited");
    await sb.join();
    await until([sa, sb], () => sa.state.phase === "open", "open");

    // My own mistake is mine to read; the loop's later successes do not take it away.
    await sa.propose({ kind: "tick", id: "nothing" }, { hold: false });
    const mine = sa.state.error;
    assert.match(mine ?? "", /^Rules: /);
    for (let i = 0; i < 3; i++) await sa.refresh();
    assert.equal(sa.state.error, mine, "still shown after three successful ticks");
    // My next action clears it.
    await sa.propose({ kind: "add", id: "milk", text: "Milk" });
    assert.notEqual(sa.state.error, mine);

    // The loop's own error is shown, and goes when the loop succeeds again.
    await until([sa, sb], () => texts(sa).includes("Milk"), "Milk");
    failListsOnce = true;
    await sa.refresh();
    assert.match(sa.state.error ?? "", /homeserver away/);
    await sa.refresh();
    assert.equal(sa.state.error, undefined, "cleared by the next successful tick");
  } finally {
    sa.stop();
    sb.stop();
  }
});

test("listeners hear every change and nothing after unsubscribe or stop()", async () => {
  const { sa, sb, stop } = await openList();
  try {
    let heard = 0;
    const off = sa.subscribe(() => heard++);
    await sa.propose({ kind: "add", id: "milk", text: "Milk" });
    assert.ok(heard >= 2, `busy on, busy off, and the view: ${heard}`);
    off();
    const afterOff = heard;
    await until([sa, sb], () => texts(sb).includes("Milk"), "Milk");
    assert.equal(heard, afterOff, "nothing after unsubscribe");

    let late = 0;
    sa.subscribe(() => late++);
    sa.stop();
    await sb.propose({ kind: "add", id: "eggs", text: "Eggs" });
    await sa.refresh();
    await new Promise((r) => setTimeout(r, 50));
    assert.equal(late, 0, "a stopped session publishes nothing");
  } finally {
    stop();
  }
});

test("a stranger sees the chain but cannot join or propose", async () => {
  const shared = new Shared();
  const a = party(shared);
  const b = party(shared);
  const c = party(shared);
  const url = await createChain("list/1", a.store, a.signer, { parties: [a.me, b.me] });
  const sc = open(c, url, shared);
  try {
    await until([sc], () => sc.state.phase === "stranger", "stranger");
    assert.deepEqual(sc.state.parties, [a.me, b.me]);
    assert.equal(sc.state.myIndex, -1);
    await sc.join();
    assert.match(sc.state.error ?? "", /NotSeated|not a party/);
    await sc.propose({ kind: "add", id: "x", text: "X" }, { hold: false });
    assert.match(sc.state.error ?? "", /NotSeated|not a party|no open seq/);
    assert.equal(sc.state.phase, "stranger");
  } finally {
    sc.stop();
  }
});
