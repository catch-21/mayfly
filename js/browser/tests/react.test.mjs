// The React hooks, rendered with react-test-renderer over the same in-memory store as the
// session tests: each hook wraps a framework-free object, and a page written through the hook
// sees the same state and the same actions.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import React from "react";
import TestRenderer from "react-test-renderer";
import { KeyedSigner } from "@synonymdev/mayfly";

import { MayflyApp, RulesRegistry, createChain, loadMayfly, shippedRules } from "../dist/index.js";
import { useChainReader, useChainSession, useCreateChain, useMyChains, useSession } from "../dist/react/index.js";
import { Shared, memoryStore } from "./memory-store.mjs";

// State arrives from listeners outside React's act(); that is how a real page gets it too.
globalThis.IS_REACT_ACT_ENVIRONMENT = false;

const WASM = readFileSync(new URL("../node_modules/@synonymdev/mayfly/pkg/mayfly_bg.wasm", import.meta.url));
await loadMayfly(WASM);

const APP = "list.example";
const app = new MayflyApp({ clientId: APP, testnet: true, wasm: WASM });
let now = 1_757_779_812_000;
const clock = () => now;

/** Render `use()` and keep its latest value; `unmount` tears the page down. */
function mount(use) {
  const box = { value: undefined, renders: 0 };
  function Probe() {
    box.value = use();
    box.renders++;
    return null;
  }
  let root;
  TestRenderer.act(() => {
    root = TestRenderer.create(React.createElement(Probe));
  });
  // Effects and their cleanups are flushed inside act(), as React schedules them.
  return { box, unmount: () => TestRenderer.act(() => root.unmount()), root };
}

async function eventually(pred, what, ms = 4000) {
  const start = Date.now();
  while (Date.now() - start < ms) {
    if (pred()) return;
    await new Promise((r) => setTimeout(r, 15));
  }
  assert.fail(`timed out waiting for ${what}`);
}

function party(shared) {
  const signer = new KeyedSigner(APP);
  return { session: null, me: signer.pubky, store: memoryStore(signer.pubky, shared), signer };
}

const texts = (h) => (h.state?.items ?? []).map((i) => i.text);

test("useChainSession follows a member from the invite to a confirmed add, and stops on unmount", async () => {
  const shared = new Shared();
  const a = party(shared);
  const b = party(shared);
  const url = await createChain("list/1", a.store, a.signer, { parties: [a.me, b.me] });
  const subs = [];
  const wake = {
    subscribe(owner, path, onEvent) {
      const s = { owner, path, open: true, onEvent };
      subs.push(s);
      return () => {
        s.open = false;
      };
    },
  };
  const options = { wake, tickMs: 30, clock };
  const A = mount(() => useChainSession(app, a, url, "list/1", options));
  const B = mount(() => useChainSession(app, b, url, "list/1", options));
  try {
    await eventually(() => B.box.value.phase === "invited", "B invited");
    assert.equal(A.box.value.phase, "waiting");
    assert.deepEqual(B.box.value.parties, [a.me, b.me]);
    assert.equal(B.box.value.myIndex, 1);
    assert.equal(typeof B.box.value.join, "function");

    await B.box.value.join();
    await eventually(() => A.box.value.phase === "open" && B.box.value.phase === "open", "both open");
    await A.box.value.propose({ kind: "add", id: "milk", text: "Milk" });
    await eventually(() => texts(B.box.value).includes("Milk"), "Milk on B");
    assert.deepEqual(texts(A.box.value), ["Milk"]);
    assert.equal(A.box.value.error, undefined);
    assert.equal(A.box.value.busy, false);
    // The hook's actions are stable across renders.
    const before = A.box.value.propose;
    await eventually(() => A.box.value.propose === before, "stable");

    // Unmounting stops the loop and lets go of the streams.
    const watchingA = subs.filter((s) => s.owner === a.me && s.open).length;
    assert.equal(watchingA, 1, "B watches A");
    B.unmount();
    assert.equal(subs.filter((s) => s.owner === a.me && s.open).length, 0, "B let go on unmount");
    const rendersAfter = B.box.renders;
    await A.box.value.propose({ kind: "add", id: "eggs", text: "Eggs" });
    await new Promise((r) => setTimeout(r, 120));
    assert.equal(B.box.renders, rendersAfter, "an unmounted page renders nothing more");
  } finally {
    A.unmount();
    B.unmount();
  }
});

test("useMyChains and useCreateChain drive the home screen", async () => {
  const shared = new Shared();
  const a = party(shared);
  const b = party(shared);
  const home = mount(() => useMyChains(app, a));
  const creating = mount(() => useCreateChain(a, "list/1"));
  try {
    await eventually(() => Array.isArray(home.box.value.chains), "chains read");
    assert.deepEqual(home.box.value.chains, []);
    assert.equal(creating.box.value.busy, false);

    const url = await creating.box.value.create({ parties: [a.me, b.me] });
    assert.ok(url.startsWith(`pubky://${a.me}/pub/${APP}/mayfly/chains/`));
    assert.equal(creating.box.value.error, undefined);
    home.box.value.reload();
    await eventually(() => home.box.value.chains?.length === 1, "one chain after reload");
    assert.deepEqual(home.box.value.chains, [{ url, finished: false }]);

    // A bad spec is reported on the hook and thrown to the caller.
    await assert.rejects(creating.box.value.create({ parties: [a.me] }), /fewer than two parties/);
    await eventually(() => /fewer than two parties/.test(creating.box.value.error ?? ""), "error shown");
    assert.equal(creating.box.value.busy, false);
  } finally {
    home.unmount();
    creating.unmount();
  }
});

test("useChainReader follows a chain and stops polling once it is final", async () => {
  const shared = new Shared();
  const a = party(shared);
  const b = party(shared);
  const url = await createChain("list/1", a.store, a.signer, { parties: [a.me, b.me] });
  const registry = new RulesRegistry(shippedRules());
  const options = { store: memoryStore("", shared), registry, url, pollMs: 25 };
  const R = mount(() => useChainReader(options));
  const idle = mount(() => useChainReader(undefined));
  try {
    assert.equal(idle.box.value.loaded, undefined);
    assert.equal(idle.box.value.final, false);
    await eventually(() => R.box.value.loaded !== undefined, "first load");
    assert.equal(R.box.value.loaded.rules, "list/1");
    assert.equal(R.box.value.final, false);
    assert.ok(R.box.value.loaded.view, R.box.value.loaded.view_error ?? "");
    assert.ok(R.box.value.loaded.loadedAt instanceof Date);

    // The chain moves; the poll picks it up without a refresh() from the page.
    const sessionOpts = { wake: shared.wake(), tickMs: 30, clock };
    const A = mount(() => useChainSession(app, a, url, "list/1", sessionOpts));
    const B = mount(() => useChainSession(app, b, url, "list/1", sessionOpts));
    try {
      await eventually(() => B.box.value.phase === "invited", "B invited");
      await B.box.value.join();
      await eventually(() => A.box.value.phase === "open", "open");
      await eventually(() => R.box.value.loaded.view?.committed.length === 1, "reader saw genesis commit");
      await A.box.value.propose({ kind: "archive" });
      await eventually(() => A.box.value.state?.archived === true, "archived");
      await B.box.value.proposeClose("finished");
      await eventually(() => A.box.value.decisions.length === 1, "A asked");
      await A.box.value.confirm(A.box.value.decisions[0].action.candidate.hash);
      await eventually(() => A.box.value.phase === "ended", "ended");
      await eventually(() => R.box.value.final === true, "reader final");
      const at = R.box.value.loaded.loadedAt;
      await new Promise((r) => setTimeout(r, 120));
      assert.equal(R.box.value.loaded.loadedAt, at, "polling stopped");
      assert.equal(typeof R.box.value.refresh, "function");
    } finally {
      A.unmount();
      B.unmount();
    }
  } finally {
    R.unmount();
    idle.unmount();
  }
});

test("useSession reports no saved session where the browser store is absent", async () => {
  const S = mount(() => useSession(app));
  try {
    assert.equal(S.box.value.session, undefined, "looking");
    await eventually(() => S.box.value.session === null, "none found");
    assert.equal(S.box.value.party, undefined);
    assert.equal(S.box.value.error, undefined);
    assert.equal(typeof S.box.value.signIn, "function");
    assert.equal(typeof S.box.value.signOut, "function");
    await S.box.value.signOut();
    assert.equal(S.box.value.session, null);
  } finally {
    S.unmount();
  }
});
