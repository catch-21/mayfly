// The browser loop, run in Node over a memory store: two members' ChainSessions and a
// ChainReader, with a shared clock and no timers. These are the behaviours the list app got
// wrong first (SKILL.md, "Rules of the loop"), pinned so the next app cannot.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { KeyedSigner } from "@synonymdev/mayfly";

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
  const me = "y".repeat(52);
  const other = "b".repeat(52);
  assert.deepEqual(parsePubkys(`${me}\n${other}, ${other.slice(0, 51)}y`, { exclude: me }), [other, `${other.slice(0, 51)}y`]);
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
    assert.match(sa.state.error ?? "", /^Oversize: record is \d+ bytes; this chain allows 65536/);
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
  assert.equal(blind.state.loaded.rulesKnown, false);
  assert.match(blind.state.loaded.viewError, /does not carry rules "tally\/1"/);
  assert.ok(blind.state.loaded.files.length > 0);

  const withRules = new RulesRegistry(shippedRules(), [tally]);
  const sighted = new ChainReader({ store: memoryStore("", shared), registry: withRules, url, pollMs: 1e9 });
  await sighted.refresh();
  assert.ok(sighted.state.loaded.view, sighted.state.loaded.viewError ?? "");
  assert.equal(sighted.state.loaded.rulesKnown, true);
});
