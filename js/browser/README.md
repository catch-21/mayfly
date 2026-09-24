# Mayfly for the browser

The client a web app drives. It does the parts every Mayfly page has to get right and none
of the parts that make a page yours: sign in with Pubky Ring, the `act()` loop with live
updates from the other members, decisions put to the person at the right moment, a proposal
held until the round takes it, the home index, and a read-only reader for anyone. You bring
the rules and the screens.

```
npm install
npm run build     # tsc → dist/
npm test          # node --test: two members and a reader over an in-memory store
```

`@synonymdev/pubky` and `@synonymdev/mayfly` are `file:` dependencies on the sibling
checkouts until they are published. Build `crates/wasm` first (`npm run build` there).

## Use

```ts
import { MayflyApp, ChainSession, ChainReader, RulesRegistry, shippedRules } from "@synonymdev/mayfly-browser";
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
written this way.

## What `ChainState` tells a page

| Field | Meaning |
| --- | --- |
| `phase` | `loading`, `stranger` (not a party), `invited` (show the terms, offer `join`), `waiting` (signed; others have not), `open`, `ended` |
| `parties`, `myIndex`, `arrangement` | The terms, from the committed genesis or, before that, from genesis as written |
| `state` | The rules state at the head; `undefined` until genesis commits |
| `pending` | Candidates of the live round only; a dead round's are already voted on |
| `decisions` | A close, a recover, or "put it forward again, or let it go" after a dead round. Answer with `confirm`, `reject`, `repropose`, `pass`, or a new proposal |
| `held` | Proposals waiting for a round that will take them; `withdraw` to take one back |
| `myTurn` | Only with `autoPass: false`: my round, nothing to carry; propose or `pass` |
| `busy`, `error` | A user action in flight; what a person should read. The loop never clears an error about the person's own action |

## What it does that a page would otherwise get wrong

- Genesis is not committed until everyone joins; the terms come from `arrangement` until then.
- A refused agreed close is consent withheld, not obstruction; the next designated proposer is
  asked whether to put it forward again, and a new proposal is the third answer.
- `open.round` is the dead round while `open.dead`; cards and pending rows use the right one.
- `AlreadyVoted`, `RoundDead`, `NotDesignated`, `Busy`, `AwaitingWitnesses` mean "wait"; a held
  proposal is retried on every tick, and a user's click waits for the loop rather than racing it.
- One event stream per other member, none for me, closed when the seats change or the page
  leaves; a browser allows about six connections to a host.
- A user action with no answer in twenty seconds gives the form back and says so.
- `index/finished` is read before `index/active`, deduplicated by URL.
