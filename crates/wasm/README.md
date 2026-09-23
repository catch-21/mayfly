# Mayfly for JavaScript

The chain client, the verifier and the views of `pubky-mayfly-client`, as one wasm-bindgen
module. Storage and signing are supplied by the caller as plain objects, so the Pubky SDK's own
npm package (`@synonymdev/pubky`) stays the only network code and a browser session never has
to cross into this module (spec §16.2.1).

```
npm run build     # wasm-pack → pkg/ (needs the wasm32-unknown-unknown target; wasm-pack via npx)
npm test          # node --test: the shared-list flow over an in-memory store, and error shapes
```

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
error variant (`AlreadyVoted`, `NoSuchCandidate`, `RoundDead`, `Busy`, `InvalidInput`, …).

The package name is provisional until the npm organisation is confirmed.
