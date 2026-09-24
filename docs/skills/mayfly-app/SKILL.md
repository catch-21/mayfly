---
name: mayfly-app
description: Build an application on Mayfly, the verifiable hashchain protocol among a small fixed set of parties on Pubky (crates pubky-mayfly, pubky-mayfly-rules, pubky-mayfly-client, pubky-mayfly-watchman). Use when writing a Mayfly app as its own project, implementing a Rules module, driving ChainClient, engaging a watchman, or testing chains over MemoryStore or a pubky-testnet. The only example app in this repository is the shared shopping list.
---

# Building on Mayfly

Mayfly is a protocol, not a service. Parties write signed, hash‑linked records to their **own**
homeserver folders; a link commits when a quorum of the parties confirms it in one voting
round; anyone replays the chain from files. The spec is `docs/MAYFLY.md`; section numbers
below refer to it. Read the section before touching what it governs.

## Layout

| Crate | Use it for |
| --- | --- |
| `pubky-mayfly` (`crates/core`) | Records, the fold (`fold::verify`), `Rules` trait, hashes/ids. No I/O. Never write records by hand from an app. |
| `pubky-mayfly-rules` | The example rules, `list/1`. A future app brings its own `Rules` implementation in its own project. |
| `pubky-mayfly-client` | `ChainClient`: the app's whole surface. `Store`/`Signer` traits with `MemoryStore`/`LocalSigner` (tests) and `PubkyStore`/`SessionSigner` (Pubky SDK). |
| `pubky-mayfly-watchman` | `Watchman` (one chain) and `Operator` (many chains, customers, credit). Run it as a service; apps only *name* a watchman in genesis. |
| `pubky-mayfly-wasm` (`crates/wasm`) | The client, verifier and views for JavaScript, over a `Store` and `Signer` the page supplies; `js/pubky-glue.js` builds both from the SDK. |
| `apps/list`, `apps/view` | The only example app: the shared shopping list for its members (`src/useList.ts` is the app loop in React; `src/ListPage.tsx` renders actions and decisions) and the read-only chain viewer for anyone (`src/mayfly.ts` is the folder walk). `docs/LIST-APP-TEST-PLAN.md` is the manual test plan they were built against. Read them; do not add the next app beside them. |
| `mayfly-demo` (`crates/demo`) | A narrated Rust walkthrough on a testnet: `src/main.rs` drives `ChainClient` and a watchman through the happy and sad paths; `src/explorer.rs` renders a chain for a bystander. Read it for the protocol, not for app structure. |

Full API cheat‑sheet: [reference.md](reference.md).

## The app loop

An app never reasons about rounds, votes or files. It does four things:

1. **Create or join.** Initiator: `ChainClient::create(rules, store, signer, GenesisSpec)`, then
   share `client.invite_url()`. Others: `ChainClient::open_url(rules, store, signer, &url)`,
   `sync()`, show the genesis (parties, quorum, witnesses, rules) to the user, then `join()`.
   Joining is consent to the fault model (§6.6) — never auto‑join.
2. **Act on every change.** `let actions = client.act().await?` whenever an event arrives
   (`store.wait_for_event` on a `PubkyStore`, `client.wait_for_change` otherwise), on a
   timer (the list uses three seconds), and after any user action. `act` syncs, mirrors, and does the honest choreography of §8.2 itself:
   confirms valid rules proposals, re‑proposes rules content after a dead round, skips a
   silent proposer after `think_ms`. A dead protocol kind (close, recover) is returned as
   `Decision { repropose: true }` for the person to answer. `act` is idempotent and never
   double‑votes.
3. **Put decisions to the user.** `Action::Decision` is a `close`, `recover` or `witnesses`
   candidate: render it, then `confirm(hash)` or `reject(hash)` (or `repropose`/`pass` if
   `repropose: true`; `confirm_abandoned` for an abandoned close). A proposal of new rules
   content in that round is the third answer; it replaces the card only once it is actually
   proposed. `Action::MyTurn { round }`: propose something or `pass()`.
   `Action::AwaitingWitnesses`: show the watchman's silence as the reason (§11.2).
   `Action::Rejected` means a link in my round was not a valid candidate and I refused it
   with evidence (§8.2 step 4) — show it; it needs no reply.
4. **Propose.** `client.propose_body(&Body::Add { .. })` for rules content. Protocol actions have
   their own methods: `propose_close(Agreed|Finished)`, `propose_abandoned(&[subject])` +
   `confirm_abandoned`, `propose_rekey(new_signer)`, `propose_recover(new_signer)`.

Read state with `client.state()?` (the rules' `State` at the head) and `client.verdict()`
(`status`, `committed`, `open`, `seats`, `engaged`, `anomalies`). Show `anomalies` — they are
attributed evidence, never something to hide.

## Rules that must survive into app code

- **Commitment consults votes only.** A watchman receipt is never a validity condition. Never
  block an append on witnesses except through `Policy::await_witnesses`, which holds *my own*
  vote and is off by default.
- **Sync before voting.** Every vote follows a fresh fold over every reachable folder; `act`
  does this. Do not cache "the current round" across events.
- **A filename is never identity.** Records are matched by BLAKE3 of their bytes. Do not parse
  paths to decide anything; the fold does it.
- **Location is authorisation, signatures are proof.** Never trust a record because of whose
  folder it sat in.
- **Genesis names a witness by pubky only.** The watchman finds the chain itself from the
  party's `index/active/<chain_id>` marker, which the client writes; no request API (§11.2).

## Writing a rules module

Implement `pubky_mayfly::rules::Rules` in the app's own project, with an id `<name>/1`.
`list/1` (`crates/rules/src/list.rs`) is the example to read. The Rust client takes the rules
as a type parameter, so a separate project depends on `pubky-mayfly` and supplies its own.
The wasm package ships `list/1` only (`AnyRules`); an app with new rules that wants the
browser client builds its own wasm module rather than extending this one.

- `type Body`: an enum with `#[serde(tag = "kind", rename_all = "snake_case")]`. The tag
  becomes the link's `kind`; `propose_body` relies on it.
- `apply` is a **pure, deterministic** function of `(state, link)`; reject anything invalid
  with `RulesError`. The fold runs it on every party's machine; two honest verifiers must reach
  identical bytes from `canonical_state`, so keep state as a plain struct with fixed field
  order and no floats, maps with unstable order, or wall‑clock reads.
- `may_append` is eligibility (who may propose what now); `obliged` is whose clock runs
  (§10, §11.3). They are independent. In the list nobody is obliged; a turn-based rules
  module can make the same party both.
- `close` turns an abandoned/agreed/finished close into an `Outcome`; `status` says when the
  state is terminal.
- `reference_hash` is pinned in genesis; a placeholder string is fine until release (§6.6).
- Set `options.time_control { think_ms, respond_ms }` in `GenesisSpec` if the rules care about
  time; otherwise both default to 24 h.

Test rules two ways: unit tests on `apply` alone, and a flow over `MemoryStore` driven by
`act()` (see `crates/client/tests/list.rs`).

## Building a web app

The shopping list is the only app in this repository. The next app is its own project: depend
on these crates (and, for a browser, on a wasm build), and read `apps/list` and `apps/view`
for the loop. Do not add it under `apps/`.

- **Dependencies.** `@synonymdev/pubky` (the SDK's `bindings/js/pkg`, built with its
  `npm run build`) and `@synonymdev/mayfly` (`crates/wasm`, built with `npm run build`) are
  `file:` dependencies until published. Rebuild the wasm package after any Rust change or the
  page runs stale code. Two build flavours: mainnet, and `build:testnet` served under
  `/testnet/` against the local homeserver (`docker compose up` at the repo root).
- **Sign in** is a Pubky Ring grant flow; the grant's client key signs every record (§5).
  `js/pubky-glue.js` turns a session into the `store` and `signer` the client wants.
  `KeyedSigner` is for Node and tests, not the browser.
- **Home is the index markers.** Your lists come from your own homeserver: `index/active/<id>`
  and `index/finished/<id>`, which the client writes and moves (§7). List finished first, then
  active, and dedupe by URL — `sync` deletes `active` when the chain is final even if this
  session never wrote it.
- **Connections are scarce.** A browser allows about six to one host, and each live event
  stream holds one. Subscribe to the other members' folders, never your own. If a subscribe
  resolves after the page has left the chain, cancel its reader. A user action that gets no
  answer in about twenty seconds should release the form and say so; the in‑flight SDK call
  cannot be cancelled.
- **Errors carry the variant name.** `error.name` is the client error (`Oversize`,
  `AlreadyVoted`, …) plus `Busy` and `InvalidInput`. Match on the name, not the message.

## Rules of the loop that the list got wrong first

**Genesis is not the chain yet.** With more than one party, genesis stays uncommitted until
the others `join()`. `state()` is `None`, `verdict().committed` is empty, and `propose_body`
refuses. Show the terms from `arrangement()` (parties, quorum, witnesses, rules) — that is
what exists before commit. `verdict().seats` can already contain the creator, so an empty
seat list is the wrong "nobody has joined" test. Validate the member field before writing:
drop self and blanks, refuse a pubky that is not z32, and refuse a duplicate rather than
merging it silently — genesis will refuse the duplicate anyway, later and less helpfully.

**A refused agreed close is consent withheld.** It is not `AnomalyKind::Obstruction` (§6.8).
Obstruction is refusing the sole valid proposal in a round. Refusing one of two competing
proposals is allowed. Refusing a finished or abandoned close still is obstruction. After the
refusal the designated proposer of the next round gets the card: put that close forward
again, or let it go. If they have already composed a new rules proposal, propose that instead
and do not show the card. An unanswered card blocks only that person's turn. When the
app's purpose is the agreement itself, that card is the feature, so design it first.

**`open.round` is the dead round while `open.dead` is true.** `Decision.round` and
`MyTurn.round` are the next round. Matching "has this person voted?" against `open.round`
hides the card, because their vote in the dead round looks like a vote in the new one.
Pending UI should list candidates of the live round only; a dead close is not still "1 of 2".

**Retry a proposal the round is not ready for.** `AlreadyVoted`, `RoundDead`,
`NotDesignated`, `Busy` and `AwaitingWitnesses` are transient, not errors to show. Hold the
user's proposal and try it on every `act()` tick, not only when the action list says
`my_turn`: the next round may belong to someone else, and waiting for your own turn drops the
proposal. Show it as waiting. Clear the queue when the page moves to another chain.

**Records over `max_body_bytes` are invalid before parsing** (default 65,536, §6.6). The
client returns `Oversize` and writes nothing; the fold drops them too. Tell the user how big
the record was and what the chain allows. Do not raise the limit to make a body fit; make the
body smaller or split it across links.

**Anomalies are attributed evidence.** Show every anomaly with an `against`. An unattributed
`UnconfirmedProposal` is only the chain still being confirmed — a member app may hide it, a
viewer should show it.

**A watchman is named by pubky in genesis and finds the chain from `index/active`.** It
needs credit (`free` or seconds) or it will not engage. Its Grant kid is minted on each
sign‑in and is not the keypair file, so a restart rotates the kid and keeps the pubky;
receipts are signed by the kid. One receipt decides nothing: with two engaged witnesses a
single outage leaves every time question asserted; three adjudicate through one. A minority
clock, however far off, does not decide. `receipt.consistent` is not a validity condition.

**The viewer is `verifyFrom`, not a second client.** It has no seat and does not vote. Poll
while the chain is open; stop when it is final, including after a refresh.

## Testing an app by hand

Write the plan before the app is finished and keep it in `docs/`; `LIST-APP-TEST-PLAN.md` is
the shape: numbered cases with written expected results, a fix list, and a session log where
a re‑run overwrites the result and appends a note. Score against the written expectation, not
against what the app did. What the list session taught about the harness:

- One browser profile per member and one for the viewer, or they share the connection limit
  and hang.
- Assert on rendered committed state (the checklist rows), not on page text: an open
  candidate's JSON contains the same words as the item it proposes.
- "No seats" is not "nobody has joined"; check that the absent member is not a seat.
- A watchman restart rotates the kid. Fail if the pubky changes, if an old receipt stops
  verifying, or if a new one fails; not because the kid changed.

## Testing pattern

```rust
let shared = MemoryStore::shared();
let identity = Keypair::random();
let signer = LocalSigner::mint(&identity, "myapp.example", NOW_S, lifetime_secs);
let store = MemoryStore::new(identity.public_key().z32(), Arc::clone(&shared));
// share one Arc<AtomicU64> clock via .with_clock(..) on every client and watchman
```

Deterministic party choice: `pubky_mayfly::vote::designated(chain, seq, round, n)` says whose
round it is — pick "the silent one" from it rather than hoping. Run everything with
`cargo test --workspace`; the `tests/testnet.rs` files run the same flows against a real
homeserver and need a Postgres (README has the Docker one‑liner).

## Not yet there

- Witness disagreement by more than `poll_ms` is specified (§11.3) and not yet compared in
  the fold. `AnomalyKind::Witness` is defined and not yet emitted (§11.6).
- No L402 or Paykit settlement in the watchman yet; `Operator::credit`/`free` is the ledger.

## Conventions

British English in prose and identifiers. No trailing whitespace. Doc comments cite the spec
section they implement. Prefer a property test or a flow test that would have caught the bug
over a unit test that asserts the fix.
