# Mayfly for JavaScript

The chain client, the verifier and the views of `pubky-mayfly-client`, as one wasm-bindgen
module. Storage and signing are supplied by the caller as plain objects, so the Pubky SDK's own
npm package (`@synonymdev/pubky`) stays the only network code and a browser session never has
to cross into this module (spec §16.2.1).

```
npm run build     # wasm-pack → pkg/ (needs the wasm32-unknown-unknown target; wasm-pack via npx)
npm test          # node --test: the shared-list flow, a JavaScript rules module, and error shapes
```

A web app does not usually drive this module directly: `@synonymdev/mayfly-browser`
(`js/browser`) is the client a page uses, and it sits on top of this one.

## Use

```js
import init, { ChainClient, verifyFrom, decodeRecord } from "@synonymdev/mayfly";
import { storeFromPubky, signerFromSession } from "@synonymdev/mayfly/js/pubky-glue.js";
await init();

// Alice creates a list and shares the invite URL.
const store = storeFromPubky(pubky, session);
const signer = await signerFromSession(session);
const alice = await ChainClient.create("list/1", store, signer, {
  parties: [alicePubky, bobPubky],
  apps: ["list.example", "list.example"],
  witnesses: [watchmanPubky],
});
const invite = alice.inviteUrl();

// Bob joins from it, after showing the genesis to the user.
const bob = ChainClient.openUrl("list/1", bobStore, bobSigner, invite);
const view = await bob.sync();            // parties, quorum, witnesses, rules
await bob.join();

// Everyone: propose typed bodies, and call act() on every change.
await alice.proposeBody({ kind: "add", id: "milk", text: "Milk" });
for (const action of await bob.act()) {
  if (action.kind === "decision") {        // a close, recover or witnesses link: ask the user
    await bob.confirm(action.candidate.hash);
  }
}
console.log(bob.state().items, bob.view().status.summary);

// Anyone: verify from the URL alone, and decode one file for the evidence panel.
const seen = await verifyFrom("list/1", storeFromPubky(pubky), invite);
const record = decodeRecord(bytes, path, etag);   // typ, payload, signature_ok, hash_matches_*
```

The store and signer shapes are in `js/pubky-glue.d.ts`; `js/pubky-glue.js` builds them from an
SDK `Pubky` and `Session`. `KeyedSigner` is a self-contained signer for Node apps and tests.
Hashes are unpadded base64url strings; errors are JS `Error`s whose `name` is the client
error variant (`AlreadyVoted`, `NoSuchCandidate`, `RoundDead`, `Busy`, `InvalidInput`, …),
with `transient: true` on the ones that mean "not yet".

Async calls on one client are queued and run one at a time. `view()`, `session()`, `state()`
and `arrangement()` are synchronous and read a snapshot taken after the last call, so a page
can render at any moment. `session()` is where a member stands (`phase`), whom to name, what
is pending in the live round, and what is held. `hold(body)` gives the client a proposal to
put forward inside `act()` when a round takes it. `setPolicy({ autoPass: true })` passes an
empty round of mine. `new ChainReader(store, resolve).load(url)` reads a chain as anyone;
`myChains(store, folder)` reads the home index.

## Your own rules

Wherever a rules id is accepted, an object is too (`src/jsrules.rs`):

```js
const tally = {
  id: "tally/1",
  referenceHash: "tally/1-reference-hash",
  init(genesis) { return { total: 0, parties: genesis.parties.map((p) => p.pubky) }; },
  mayAppend(state, party, kind) { return kind === "add"; },
  apply(state, link) { return { ...state, total: state.total + link.body.n }; },   // throw to refuse
  close(state, close) { return { summary: `closed at ${state.total}` }; },
  // optional: wantsReveals(genesis), obliged(state), status(state), canonicalState(state)
};
const alice = await ChainClient.create(tally, store, signer, { parties });
const seen = await verifyFrom(tally, readOnlyStore, invite);
```

Every method is synchronous and must be a pure function of its arguments; the verifier runs
`apply` on every party's machine and expects identical `canonicalState` bytes (default: JSON
with sorted keys). `link.author` is a pubky; keep `genesis.parties` in the state to find a
seat. `tests/list.test.mjs` runs this `tally/1` end to end.

The package name is provisional until the npm organisation is confirmed.
