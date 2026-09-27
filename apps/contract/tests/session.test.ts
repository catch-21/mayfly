// Two parties and a reader, over an in-memory store: offer, revise, accept, reject, and a
// refused walk-away. The loop is the browser client's; these tests only drive contract/1.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { KeyedSigner } from "@synonymdev/mayfly";
import {
  ChainReader,
  ChainSession,
  RulesRegistry,
  createChain,
  loadMayfly,
  myChains,
} from "../../../js/browser/dist/core.js";
import { Shared, memoryStore } from "../../../js/browser/tests/memory-store.mjs";

import { contractRules, type ContractBody, type ContractState } from "../src/rules.ts";

await loadMayfly(readFileSync(new URL("../../../crates/wasm/pkg/mayfly_bg.wasm", import.meta.url)));

const APP = "contract.mayfly.example";
const FOLDER = `/pub/${APP}/mayfly/`;
const THINK_MS = 60_000;

let now = 1_757_779_812_000;
const clock = () => now;

type Session = ChainSession<ContractState>;

async function until(sessions: Session[], pred: () => boolean, what: string) {
  for (let i = 0; i < 40; i++) {
    for (const s of sessions) await s.refresh();
    if (pred()) return;
    now += THINK_MS + 1;
  }
  assert.fail(`timed out waiting for ${what}: ${JSON.stringify(sessions.map((s) => s.state.phase))}`);
}

function party(shared: Shared) {
  const signer = new KeyedSigner(APP);
  return { signer, store: memoryStore(signer.pubky, shared), me: signer.pubky };
}

function open(p: ReturnType<typeof party>, url: string, shared: Shared) {
  const s = new ChainSession<ContractState>({
    rules: contractRules,
    store: p.store,
    signer: p.signer,
    url,
    wake: shared.wake(),
    tickMs: 1e9,
    actionTimeoutMs: 5_000,
    clock,
  });
  s.start();
  return s;
}

async function twoMembers() {
  const shared = new Shared();
  const a = party(shared);
  const b = party(shared);
  const url = await createChain(contractRules, a.store, a.signer, {
    parties: [a.me, b.me],
    options: { time_control: { think_ms: THINK_MS, respond_ms: THINK_MS } },
  });
  const sa = open(a, url, shared);
  const sb = open(b, url, shared);
  await until([sa, sb], () => sb.state.phase === "invited", "invited");
  await sb.join();
  await until([sa, sb], () => sa.state.phase === "open" && sb.state.phase === "open", "open");
  return { shared, a, b, url, sa, sb, stop: () => (sa.stop(), sb.stop()) };
}

function offerText(s: Session): string | undefined {
  return s.state.state?.offer?.text;
}

/** Accept and reject are not held: they name the offer open now. Retry while the round is not ours. */
async function answerNow(s: Session, body: ContractBody) {
  for (let i = 0; i < 20; i++) {
    await s.propose(body, { hold: false });
    const err = s.state.error;
    if (!err) return;
    if (!err.includes("settling")) assert.fail(err);
    now += THINK_MS + 1;
    await s.refresh();
  }
  assert.fail(`could not propose ${body.kind}`);
}

async function seal(sa: Session, sb: Session) {
  await sa.proposeClose("finished");
  await until([sa, sb], () => sa.state.decisions.length + sb.state.decisions.length > 0, "close asked");
  for (const s of [sa, sb]) {
    const d = s.state.decisions.find((x) => !x.action.repropose);
    if (d) await s.confirm(d.action.candidate.hash);
  }
  await until([sa, sb], () => sa.state.phase === "ended" && sb.state.phase === "ended", "ended");
}

test("an offer, a revision, and an accept become a finished agreed contract", async () => {
  const { shared, a, b, url, sa, sb, stop } = await twoMembers();
  try {
    const registry = new RulesRegistry([], [contractRules]);
    const reader = new ChainReader<ContractState>({
      store: memoryStore("", shared),
      registry,
      url,
      pollMs: 1e9,
    });
    await reader.refresh();
    assert.equal(reader.state.loaded?.rules, "contract/1");
    assert.equal(reader.state.final, false);

    await sa.propose({ kind: "offer", id: "1", text: "Pay 10", base: "" });
    await until([sa, sb], () => offerText(sb) === "Pay 10", "opening offer");
    await sb.propose({ kind: "offer", id: "2", text: "Pay 12", base: "1" });
    await until([sa, sb], () => offerText(sa) === "Pay 12" && sa.state.state?.offer?.previous === "Pay 10", "revision");
    await answerNow(sa, { kind: "accept", id: "2" });
    await until([sa, sb], () => sb.state.state?.outcome?.kind === "agreed", "accepted");
    assert.equal(sa.state.state?.outcome?.text, "Pay 12");
    assert.equal(sa.state.state?.offer, null);

    await seal(sa, sb);
    assert.equal(sa.state.view?.status.outcome, "agreed");
    assert.deepEqual(await myChains(a.store, FOLDER), [{ url, finished: true }]);
    assert.deepEqual(await myChains(b.store, FOLDER), [{ url, finished: true }]);
    await reader.refresh();
    assert.equal(reader.state.final, true);
    assert.equal(reader.state.loaded?.view?.state?.outcome?.text, "Pay 12");
  } finally {
    stop();
  }
});

test("an outright reject ends with no contract once the finished close is confirmed", async () => {
  const { sa, sb, stop } = await twoMembers();
  try {
    await sa.propose({ kind: "offer", id: "1", text: "Pay 10", base: "" });
    await until([sa, sb], () => offerText(sb) === "Pay 10", "offer");
    await answerNow(sb, { kind: "reject", id: "1" });
    await until([sa, sb], () => sa.state.state?.outcome?.kind === "rejected", "rejected");
    assert.equal(sa.state.state?.outcome?.text, "Pay 10");
    await seal(sa, sb);
    assert.equal(sa.state.view?.status.outcome, "rejected");
  } finally {
    stop();
  }
});

test("letting a refused close go ends the negotiation once, and the question does not rotate", async () => {
  const { sa, sb, stop } = await twoMembers();
  try {
    await sa.propose({ kind: "offer", id: "1", text: "Pay 10", base: "" });
    await until([sa, sb], () => offerText(sb) === "Pay 10", "offer");
    await sa.proposeClose("agreed");
    await until([sa, sb], () => sb.state.decisions.length === 1, "B is asked");
    await sb.reject(sb.state.decisions[0].action.candidate.hash);
    await until(
      [sa, sb],
      () => sa.state.decisions.some((d) => d.action.repropose) || sb.state.decisions.some((d) => d.action.repropose),
      "carry-forward card",
    );
    const who = [sa, sb].find((s) => s.state.decisions.some((d) => d.action.repropose));
    if (!who) assert.fail("no carry-forward card");
    await who.propose({ kind: "lapse" });
    await until([sa, sb], () => sa.state.state?.outcome?.kind === "lapsed" && sb.state.state?.outcome?.kind === "lapsed", "lapsed");
    assert.equal(sa.state.state?.offer, null);
    assert.equal(sa.state.state?.outcome?.text, "Pay 10");
    for (let i = 0; i < 4; i++) {
      now += THINK_MS + 1;
      await sa.refresh();
      await sb.refresh();
    }
    assert.equal(sa.state.decisions.length, 0, "the close question does not come back");
    assert.equal(sb.state.decisions.length, 0, "nor does it move to the other party");
    assert.equal(sa.state.phase, "open");
    await seal(sa, sb);
    assert.equal(sa.state.view?.status.outcome, "no contract");
    assert.equal(sa.state.decisions.length, 0);
    assert.equal(sb.state.decisions.length, 0);
  } finally {
    stop();
  }
});

test("refusing to walk away leaves the offer open and is not obstruction", async () => {
  const { sa, sb, stop } = await twoMembers();
  try {
    await sa.propose({ kind: "offer", id: "1", text: "Pay 10", base: "" });
    await until([sa, sb], () => offerText(sb) === "Pay 10", "offer");
    await sa.proposeClose("agreed");
    await until([sa, sb], () => sb.state.decisions.length === 1, "B is asked");
    const ask = sb.state.decisions[0].action;
    assert.equal(ask.candidate.kind, "close");
    assert.equal(ask.repropose, false);
    await sb.reject(ask.candidate.hash);
    await until([sa, sb], () => sa.state.phase === "open" && offerText(sa) === "Pay 10", "offer still open");
    assert.equal(sb.state.state?.offer?.text, "Pay 10");
    assert.equal(sa.state.state?.outcome, null);
    assert.ok(!sa.state.view?.anomalies.some((x) => x.kind === "Obstruction"), JSON.stringify(sa.state.view?.anomalies));
    assert.ok(!sb.state.view?.anomalies.some((x) => x.kind === "Obstruction"));
  } finally {
    stop();
  }
});
