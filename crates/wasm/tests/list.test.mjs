// The shared list (`list/1`) run through the wasm module the way a web app would run it:
// parties join from an invite URL, propose typed bodies, and otherwise only call `act()`.
// Mirrors crates/client/tests/list.rs. Run `npm run build` first.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import init, {
  ChainClient,
  KeyedSigner,
  verifyFrom,
  decodeRecord,
  parseChainUrl,
  chainUrl,
  rulesIds,
  designatedProposer,
} from "../pkg/mayfly.js";
import { Shared, memoryStore } from "./memory-store.mjs";

await init({ module_or_path: readFileSync(new URL("../pkg/mayfly_bg.wasm", import.meta.url)) });

const THINK_MS = 60_000;

/** A controllable clock shared by every client. */
class Clock {
  constructor(ms) {
    this.ms = ms;
  }
  reader() {
    return () => this.ms;
  }
  advance(ms) {
    this.ms += ms;
  }
}

async function everyone(clients, present) {
  const out = clients.map(() => []);
  for (const i of present) out[i] = await clients[i].act();
  return out;
}

/** Act until `len` links are committed, advancing the clock past `think_ms` each round. */
async function settle(clients, present, clock, len) {
  const seen = [];
  for (let i = 0; i < 12; i++) {
    for (const acts of await everyone(clients, present)) seen.push(...acts);
    if (clients[present[0]].view().committed.length >= len) return seen;
    clock.advance(THINK_MS + 1);
  }
  assert.fail(`did not settle: ${JSON.stringify(seen)}`);
}

const items = (c) => (c.state() ? c.state().items.map((i) => i.text) : null);

/** Byte order of two base64url hashes, as the protocol compares them (§6.4), not string order. */
function lower(a, b) {
  const bytes = (s) => Buffer.from(s, "base64url");
  return Buffer.compare(bytes(a), bytes(b)) <= 0 ? a : b;
}

test("a shared list runs on act() alone, through the wasm module", async () => {
  assert.deepEqual(rulesIds(), ["list/1"]);

  const shared = new Shared();
  const clock = new Clock(1_757_779_812_000);
  const apps = ["list.example", "list.example", "other.example"];
  const signers = apps.map((app) => new KeyedSigner(app));
  const stores = signers.map((s) => memoryStore(s.pubky, shared));
  const pubkies = signers.map((s) => s.pubky);

  // Alice creates the list and sends an invite URL; Bob and Carol join from it.
  const alice = await ChainClient.create("list/1", stores[0], signers[0], {
    parties: pubkies,
    apps,
    options: { time_control: { think_ms: THINK_MS, respond_ms: THINK_MS } },
  });
  alice.setClock(clock.reader());
  const invite = alice.inviteUrl();
  assert.ok(invite.startsWith(`pubky://${pubkies[0]}/pub/list.example/mayfly/chains/`), invite);
  const parsed = parseChainUrl(invite);
  assert.equal(parsed.owner, pubkies[0]);
  assert.equal(parsed.folder, "/pub/list.example/mayfly/");
  assert.equal(chainUrl(parsed.owner, parsed.folder, parsed.chain), invite);
  assert.equal(alice.chain(), parsed.chain);

  const clients = [alice];
  for (let i = 1; i < 3; i++) {
    const c = ChainClient.openUrl("list/1", stores[i], signers[i], invite);
    c.setClock(clock.reader());
    await c.sync();
    await c.join();
    clients.push(c);
  }
  const all = [0, 1, 2];
  await settle(clients, all, clock, 1);
  for (const c of clients) {
    assert.equal(c.view().status.kind, "ongoing");
    assert.deepEqual(items(c), []);
    assert.deepEqual(c.view().parties, pubkies);
    assert.equal(c.view().rules, "list/1");
  }
  assert.equal(clients[1].myIndex(), 1);

  // An append: the author proposes a typed body; the others confirm inside act().
  const l1 = await clients[0].proposeBody({ kind: "add", id: "milk", text: "Milk" });
  const acts = await settle(clients, all, clock, 2);
  assert.equal(acts.filter((a) => a.kind === "confirmed" && a.hash === l1).length, 2);
  for (const c of clients) assert.deepEqual(items(c), ["Milk"]);
  const head = clients[0].view().committed.at(-1);
  assert.equal(head.hash, l1);
  assert.equal(head.kind, "add");
  assert.deepEqual(head.body, { id: "milk", text: "Milk" });
  assert.equal(head.author, 0);
  assert.deepEqual([...head.confirmers].sort(), [1, 2]);

  // Competing proposals converge through a dead round; a proposer who walks away is skipped
  // after think_ms. Two of three cannot commit under unanimity; the walker's return does.
  const seq = clients[0].view().open.seq;
  const before = clients[0].view().committed.length;
  const la = await clients[0].proposeBody({ kind: "add", id: "eggs", text: "Eggs" });
  const lb = await clients[1].proposeBody({ kind: "add", id: "bread", text: "Bread" });
  assert.notEqual(la, lb);
  // The walker is whoever round 1 falls to, so the skip is exercised, not left to chance.
  const walker = designatedProposer(alice.chain(), seq, 1, 3);
  const present = all.filter((p) => p !== walker);
  const saw = [];
  for (let i = 0; i < 4; i++) {
    for (const a of await everyone(clients, present)) saw.push(...a);
    clock.advance(THINK_MS + 1);
  }
  assert.ok(saw.some((a) => a.kind === "skipped"), `a silent designated proposer was skipped: ${JSON.stringify(saw)}`);
  assert.ok(
    saw.some((a) => a.kind === "reproposed" && a.earlier === lower(la, lb)),
    `the lowest-hash content was re-proposed: ${JSON.stringify(saw)}`,
  );
  assert.equal(clients[present[0]].view().committed.length, before, "one vote short");
  const back = await clients[walker].act();
  assert.ok(back.some((a) => a.kind === "confirmed"), JSON.stringify(back));
  await settle(clients, all, clock, before + 1);
  const winner = lower(la, lb) === la ? "Eggs" : "Bread";
  for (const c of clients) assert.deepEqual(items(c), ["Milk", winner]);
  assert.equal(clients[0].view().open.seq, seq + 1);

  // Ticking is an ordinary append; closing is a decision for the user.
  await clients[1].proposeBody({ kind: "tick", id: "milk" });
  await settle(clients, all, clock, before + 2);
  assert.equal(clients[2].state().items[0].ticked, true);
  const close = await clients[2].proposeClose("agreed");
  const decisions = await everyone(clients, all);
  for (const p of [0, 1]) {
    assert.equal(decisions[p].length, 1, JSON.stringify(decisions[p]));
    assert.equal(decisions[p][0].kind, "decision");
    assert.equal(decisions[p][0].candidate.hash, close);
    assert.equal(decisions[p][0].repropose, false);
    await clients[p].confirm(close);
  }
  await settle(clients, all, clock, before + 3);
  for (const c of clients) {
    const v = c.view();
    assert.equal(v.is_final, true);
    assert.equal(v.status.kind, "closed");
    assert.equal(v.open, null);
  }

  // A bystander verifies from the invite URL alone and sees the same chain.
  const observer = memoryStore("", shared);
  const seen = await verifyFrom("list/1", observer, invite);
  assert.deepEqual(
    seen.committed.map((l) => l.hash),
    clients[0].view().committed.map((l) => l.hash),
  );
  assert.deepEqual(seen.suspects, []);

  // Every file decodes; a tampered mirror is caught by the view and by the verifier.
  const bobPaths = shared.paths(pubkies[1]).filter((p) => p.includes("/links/00000001-"));
  assert.equal(bobPaths.length, 1);
  const bytes = shared.files.get(`pubky://${pubkies[1]}${bobPaths[0]}`);
  const record = decodeRecord(bytes, bobPaths[0]);
  assert.equal(record.typ, "mayfly-link");
  assert.equal(record.signature_ok, true);
  assert.equal(record.hash_matches_name, true);
  assert.equal(record.payload.kind, "add");
  assert.equal(record.payload.confirms.length, 2, "genesis QC embedded");
  assert.equal(record.payload.confirms[0].typ, "mayfly-confirm");
  shared.tamper(pubkies[1], bobPaths[0], (b) => {
    const at = b.lastIndexOf(".".charCodeAt(0)) + 1;
    b[at] = b[at] === 65 ? 66 : 65; // 'A' <-> 'B' in the signature
    return b;
  });
  const tampered = decodeRecord(shared.files.get(`pubky://${pubkies[1]}${bobPaths[0]}`), bobPaths[0]);
  assert.equal(tampered.signature_ok, false);
  assert.equal(tampered.hash_matches_name, false);
  const after = await verifyFrom("list/1", observer, invite);
  assert.deepEqual(after.committed.map((l) => l.hash), seen.committed.map((l) => l.hash), "originals stand");
  assert.ok(
    after.anomalies.some((a) => a.kind === "TamperedMirror") || after.suspects.length > 0,
    JSON.stringify({ anomalies: after.anomalies, suspects: after.suspects }),
  );
});

test("errors are JS Errors named after the variant", async () => {
  const shared = new Shared();
  const signer = new KeyedSigner("list.example");
  const other = new KeyedSigner("list.example");
  const store = memoryStore(signer.pubky, shared);
  const parties = [signer.pubky, other.pubky];
  await assert.rejects(
    ChainClient.create("chess/1", store, signer, { parties }),
    (e) => e.name === "InvalidInput" && /unknown rules/.test(e.message),
  );
  await assert.rejects(
    ChainClient.create("list/1", store, signer, { parties: [signer.pubky] }),
    (e) => e.name === "Core" && /fewer than two parties/.test(e.message),
  );
  assert.throws(
    () => ChainClient.openUrl("list/1", store, signer, "https://example.com/not-a-chain"),
    (e) => e.name === "State",
  );
  const c = await ChainClient.create("list/1", store, signer, { parties });
  await c.act();
  // Genesis is the initiator's own vote at seq 0; voting again there is the §6.4 discipline.
  await assert.rejects(c.confirm("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"), (e) => e.name === "AlreadyVoted");
  await assert.rejects(c.confirm(c.view().open.candidates[0].hash), (e) => e.name === "AlreadyVoted");
  await assert.rejects(c.confirm("not a hash"), (e) => e.name === "InvalidInput");
  assert.throws(() => ChainClient.openUrl("list/1", store, { pubky: "x" }, c.inviteUrl()), (e) => e.name === "InvalidInput");
  assert.throws(() => designatedProposer(c.chain(), 1, 0, 3), (e) => e.name === "InvalidInput");
});
