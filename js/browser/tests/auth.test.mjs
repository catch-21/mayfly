// `MayflyApp` without a homeserver: what it derives from its options, what it refuses, and
// how it behaves where the browser's session store is not there. Signing in with Ring, the
// testnet shortcut against a live homeserver, and `party(session)` need a homeserver and are
// covered by the manual plan (docs/LIST-APP-TEST-PLAN.md, section A).

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { MayflyApp, isPubky, loadMayfly, pubkyWake } from "../dist/index.js";

const WASM = readFileSync(new URL("../node_modules/@synonymdev/mayfly/pkg/mayfly_bg.wasm", import.meta.url));

test("an app knows its folder, its capabilities and its network", async () => {
  const app = new MayflyApp({ clientId: "list.example", testnet: true, wasm: WASM });
  assert.equal(app.clientId, "list.example");
  assert.equal(app.testnet, true);
  assert.equal(app.folder, "/pub/list.example/mayfly/");
  assert.equal(app.capabilities, "/pub/list.example/:rw");
  await app.ready();
  await app.ready();
  const main = new MayflyApp({ clientId: "x.example" });
  assert.equal(main.testnet, false);
  assert.equal(main.folder, "/pub/x.example/mayfly/");
});

test("the sign-up shortcut is refused off the testnet", async () => {
  const main = new MayflyApp({ clientId: "x.example", wasm: WASM });
  await assert.rejects(main.testnetSignUp(), /testnet only/);
});

test("the sign-in QR is a PNG data URL of the flow's authorization URL", async () => {
  const app = new MayflyApp({ clientId: "list.example", testnet: true, wasm: WASM });
  const qr = await app.qrDataUrl({ authorizationUrl: "pubkyauth:///?relay=x&secret=y" }, 120);
  assert.match(qr, /^data:image\/png;base64,/);
  assert.ok(qr.length > 200);
});

test("without a browser session store there is nothing to restore and nothing to forget", async () => {
  const app = new MayflyApp({ clientId: "list.example", testnet: true, wasm: WASM });
  assert.equal(await app.restore(), undefined);
  await app.forget();
  await app.remember({ info: { clientId: "list.example" } });
});

test("a read-only store has no owner and can be handed to the reader", async () => {
  const app = new MayflyApp({ clientId: "list.example", testnet: true, wasm: WASM });
  const store = await app.readOnlyStore();
  assert.equal(store.me, "");
  for (const f of ["list", "get", "put", "delete"]) assert.equal(typeof store[f], "function");
  await assert.rejects(store.put("/pub/x", new Uint8Array()), /read-only/);
});

test("a wake over the SDK returns an unsubscribe that is safe before the stream opens", async () => {
  await loadMayfly(WASM);
  const app = new MayflyApp({ clientId: "list.example", testnet: true, wasm: WASM });
  const wake = pubkyWake(app.pubky);
  // No homeserver is reachable here: the subscription fails quietly and the loop's timer
  // covers it. Unsubscribing at once must not throw either.
  const off = wake.subscribe("8pinxxgqs41n4aididenw5apqp1urfmzdztr8jt4abrkdn435ewo", "/pub/list.example/mayfly/", () => {
    assert.fail("no event without a homeserver");
  });
  off();
  await new Promise((r) => setTimeout(r, 50));
});

test("isPubky is the module's judgement, not a regex", () => {
  assert.ok(isPubky("8pinxxgqs41n4aididenw5apqp1urfmzdztr8jt4abrkdn435ewo"));
  assert.ok(!isPubky("8pinxxgqs41n4aididenw5apqp1urfmzdztr8jt4abrkdn435ew"));
  assert.ok(!isPubky(""));
  assert.ok(!isPubky("not a pubky"));
});
