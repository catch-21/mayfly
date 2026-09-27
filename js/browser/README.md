# Mayfly for the browser

The client a web app drives. It is thin on purpose: everything about the protocol — the
`act()` loop, held proposals, decisions, where a member stands, the home index, the reader's
folder walk and verification — is the Rust client, reached through the wasm module. What this
package adds is what a browser has and the module does not: sign-in with Pubky Ring, event
streams from the other members' homeservers, a timer, a timeout that gives the form back, and
what a person is told. You bring the rules and the screens.

```
npm install
npm run build     # tsc → dist/
npm test          # node --test, over an in-memory store: the session loop, the React hooks, MayflyApp
```

The tests are in three files. `tests/session.test.mjs` runs two members' `ChainSession`s and a
`ChainReader` through the protocol (phases, held proposals, decisions, competing proposals,
oversize, close) and through what this package owns (the wake's connection budget, the timer,
the timeout, which errors stay, listeners, a stranger). `tests/react.test.mjs` renders every
hook with `react-test-renderer` over the same store. `tests/auth.test.mjs` covers `MayflyApp`
without a homeserver. Signing in with Ring and the testnet shortcut against a live homeserver
are in the manual plan, `docs/LIST-APP-TEST-PLAN.md`.

`@synonymdev/pubky` and `@synonymdev/mayfly` are `file:` dependencies on the sibling
checkouts until they are published. Build `crates/wasm` first (`npm run build` there).

## Use

```ts
import { MayflyApp, ChainSession, ChainReader, RulesRegistry, shippedRules, pubkyWake } from "@synonymdev/mayfly-browser";
import wasmUrl from "@synonymdev/mayfly/pkg/mayfly_bg.wasm?url";   // Vite

// This app on this network. The client id names the folder every record lives under.
const app = new MayflyApp({ clientId: "list.mayfly.example", testnet: true, wasm: wasmUrl });

// Sign in: a Ring grant flow shown as a QR, remembered in the browser store.
const flow = await app.startRingSignIn();
show(await app.qrDataUrl(flow));
const session = await flow.awaitApproval();
await app.remember(session);
const party = await app.party(session);          // { me, store, signer }

// Open a chain and keep it honest. `state` is everything a page shows.
const chain = new ChainSession<ListState, ListBody>({
  rules: "list/1", store: party.store, signer: party.signer, url: inviteUrl, wake: pubkyWake(app.pubky),
});
chain.subscribe((s) => render(s));
chain.start();
await chain.join();                                    // consent to the terms (§8.1)
await chain.propose({ kind: "add", id, text });        // held until the round takes it
for (const d of chain.state.decisions) await chain.confirm(d.action.candidate.hash);
chain.stop();

// Read a chain as anyone.
const reader = new ChainReader({ store: await app.readOnlyStore(), registry: new RulesRegistry(shippedRules()), url });
reader.subscribe((s) => renderFiles(s.loaded));
reader.start();                                        // polls until the chain is final
```

React apps use the hooks in `@synonymdev/mayfly-browser/react`: `useSession(app)`,
`useRingSignIn(app, onSession)`, `useMyChains(app, party)`, `useCreateChain(party, rules)`,
`useChainSession(app, party, url, rules)` and `useChainReader(options)`. Each wraps the object
above; `apps/list` and `apps/view` are the worked examples.

## Your own rules

Rules the wasm module does not ship are a plain object (`RulesModule`): `id`,
`referenceHash`, `init`, `mayAppend`, `apply`, `close`, and optionally `wantsReveals`,
`obliged`, `status`, `canonicalState`. Every method is synchronous and pure; throw to refuse.
Pass the object wherever a rules id is accepted, and add it to a `RulesRegistry` so the
reader can verify chains that name it. `crates/wasm/tests/list.test.mjs` runs a `tally/1`
written this way. `hashBytes` is the protocol's BLAKE3 (unpadded base64url) for bytes the
app names itself, such as a file kept off the chain.

## What `ChainState` tells a page

| Field | Meaning |
| --- | --- |
| `phase` | `loading`, `stranger` (not a party), `invited` (show the terms, offer `join`), `waiting` (signed; others have not), `open`, `ended`. Decided by the client. |
| `parties`, `myIndex`, `arrangement` | The terms, from the committed genesis or, before that, from genesis as written |
| `state` | The rules state at the head; `undefined` until genesis commits |
| `pending` | Candidates of the live round only; a dead round's are already voted on |
| `decisions` | What the client asked on its last step: a close, a recover, or "put it forward again, or let it go" after a dead round. Answer with `confirm`, `reject`, `repropose`, `pass`, or a new proposal |
| `held` | Proposals the client is holding for a round that will take them; `withdraw` to take one back |
| `myTurn` | Only with `autoPass: false`: my round, nothing to carry; propose or `pass` |
| `busy`, `error` | A user action in flight; what a person should read. The loop never clears an error about the person's own action |

## Where the safety lives

The rules of the loop that the first web app got wrong are in the Rust client now, so every
binding gets them (`crates/client/tests/list.rs` pins them; `tests/session.test.mjs` pins
them again through this package):

- Genesis is not committed until everyone joins; `phase` and `parties` say so.
- A held proposal is retried inside `act()` on every call, whichever member the round belongs
  to; when it goes out it answers a "put it forward again?" question, and a refusal for good
  is reported once and dropped.
- `pending` is the live round only; `open.round` is the dead round while `open.dead`.
- Errors that mean "not yet" carry `transient: true`; the module decides which.
- Every call into the client is queued, so a click during a tick waits rather than failing.

What stays here, and why: one event stream per other member and none for me, closed when the
seats change or the page leaves (a browser allows about six connections to a host); a user
action with no answer in twenty seconds gives the form back, since the SDK call cannot be
cancelled.
