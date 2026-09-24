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
| `pubky-mayfly-wasm` (`crates/wasm`) | The client, verifier and views for JavaScript, over a `Store` and `Signer` the page supplies; `js/pubky-glue.js` builds both from the SDK. Rules are a shipped id or an object the app writes in JavaScript. |
| `@synonymdev/mayfly-browser` (`js/browser`) | **The client a web app drives.** Sign-in, the `act()` loop, decisions, held proposals, live updates, the home index, a read-only reader; React hooks under `/react`. Start here for a browser app. |
| `apps/list`, `apps/view` | The only example app: the shared shopping list for its members (`src/useList.ts` names the list's verbs over `useChainSession`; `src/ListPage.tsx` renders the phases and decisions) and the read-only chain viewer for anyone (over `useChainReader`). `docs/LIST-APP-TEST-PLAN.md` is the manual test plan they were built against. Read them; do not add the next app beside them. |
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
   `Decision { repropose: true }` for the person to answer. Then it tries the oldest
   proposal given to `hold()` (`Action::Proposed` when it goes out, `Action::HeldRefused`
   when the rules or the size limit refuse it for good), and, with `policy.auto_pass`, passes
   an empty round of mine. `act` is idempotent and never double‑votes. `phase()` and
   `session_view()` say where the party stands and what a page shows between calls.
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

Rules live in the app's own project, with an id `<name>/1`. Two ways to write them:

- **In Rust**, implement `pubky_mayfly::rules::Rules`; `list/1` (`crates/rules/src/list.rs`)
  is the example. The Rust client takes the rules as a type parameter, so a separate project
  depends on `pubky-mayfly` and supplies its own. The wasm package ships `list/1` only.
- **In JavaScript**, write a `RulesModule` object (`js/browser/src/rules.ts`): `id`,
  `referenceHash`, `init`, `mayAppend`, `apply`, `close`, and optionally `wantsReveals`,
  `obliged`, `status`, `canonicalState`. Pass it wherever a rules id is accepted; no wasm
  rebuild. `crates/wasm/tests/list.test.mjs` runs a `tally/1` written this way. `link.author`
  is a pubky, so `init` keeps `genesis.parties` in the state to find a seat.

Either way:

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

The shopping list is the only app in this repository. The next app is its own project that
depends on `@synonymdev/mayfly-browser` (and, for Rust rules, on a wasm build). Read
`apps/list` and `apps/view` for the shape. Do not add it under `apps/`.

Use the browser client; do not drive the wasm `ChainClient` from a page yourself. The
protocol logic — the loop, held proposals, decisions, the phase, the home index, the reader —
is the Rust client's and reaches the page through the wasm module; the browser package adds
only what a browser has (sign-in, event streams, a timer, a timeout). What it gives you
(`js/browser/README.md` has the table):

- **`MayflyApp`**: one app on one network. Ring sign-in as a QR, the testnet shortcut,
  remember/restore/forget, `party(session)` → the store and signer, `readOnlyStore()`.
- **`ChainSession`** (`useChainSession` in React): `phase` (`loading`, `stranger`,
  `invited`, `waiting`, `open`, `ended`), `parties` and `arrangement` before commit, `state`,
  `pending` (live round only), `decisions`, `held`, `busy`, `error`; `join`, `propose`,
  `confirm`, `reject`, `repropose`, `pass`, `proposeClose`, `proposeAbandoned`.
- **`ChainReader`** (`useChainReader`): the folder walk, the verifier, decoded files, polling
  that stops when the chain is final. A `RulesRegistry` says which rules it can run.
- **`myChains`, `createChain`, `parsePubkys`**: the home screen.

Your page owns: the rules and their types, the screens for each `phase`, how a decision card
reads, and what a proposal form is. `propose(body)` is held by default until the round takes
it; pass `{ hold: false }` for actions on the current state (a tick, an edit) that should
not wait. Set `autoPass: false` for rules where a round of yours is a move to make.

- **Dependencies.** `@synonymdev/pubky` (the SDK's `bindings/js/pkg`, built with its
  `npm run build`), `@synonymdev/mayfly` (`crates/wasm`, `npm run build`) and
  `@synonymdev/mayfly-browser` (`js/browser`, `npm run build`) are `file:` dependencies until
  published. Rebuild after a change in any of them or the page runs stale code. Pass the wasm
  URL to `MayflyApp` (Vite: `?url` import). Two build flavours: mainnet, and `build:testnet`
  served under `/testnet/` against the local homeserver (`docker compose up`).
- **Errors carry the variant name.** `error.name` is the client error (`Oversize`,
  `AlreadyVoted`, …) plus `Busy` and `InvalidInput`. Match on the name, not the message.

## What the client does for you, and why

These are the rules of the loop the list got wrong first. The Rust client now holds them,
so every binding gets them; read them so your screens do not fight it.
`crates/client/tests/list.rs` pins each one, and `js/browser/tests/session.test.mjs` pins
them again through the browser package.

**Genesis is not the chain yet.** With more than one party, genesis stays uncommitted until
the others `join()`. `state` is `undefined`, `view.parties` is empty, and a proposal is
refused. `phase` is `invited` or `waiting`, and `parties` come from the `arrangement`. The
creator can already be a seat, so an empty seat list is the wrong "nobody has joined" test.
`parsePubkys` refuses a repeat or a non-pubky by name before genesis is written.

**A refused agreed close is consent withheld.** It is not `AnomalyKind::Obstruction` (§6.8).
Obstruction is refusing the sole valid proposal in a round. Refusing one of two competing
proposals is allowed. Refusing a finished or abandoned close still is obstruction. After the
refusal the designated proposer of the next round gets the card: put that close forward
again, or let it go — that can be the person who refused it. A held proposal is the third
answer and the card is not shown. An unanswered card blocks only that person's turn. When
the app's purpose is the agreement itself, that card is the feature, so design it first.

**`open.round` is the dead round while `open.dead` is true.** `Decision.round` and
`MyTurn.round` are the next round. The client matches "has this person voted?" against the
decision's round, and `pending` lists the live round only; a dead close is not still "1 of 2".

**A proposal the round is not ready for waits.** `Error::is_transient` names the errors that
mean "not yet" (`AlreadyVoted`, `RoundDead`, `NotDesignated`, `AwaitingWitnesses`; in
JavaScript `e.transient`, which also covers `Busy`). `hold()` gives the client a proposal to
retry inside `act()`, whichever member the round belongs to. The wasm client queues every
call, so a click during a tick waits rather than failing. The loop never clears an error
about the person's own action.

**Records over `max_body_bytes` are invalid before parsing** (default 65,536, §6.6). The
client returns `Oversize` and writes nothing; the fold drops them too. Tell the user how big
the record was and what the chain allows. Do not raise the limit to make a body fit; make the
body smaller or split it across links.

**Competing proposals converge without anyone.** Two members proposing at once kill the
round; the next designated proposer carries the lowest-hash link forward inside `act()`. The
other proposal is not re-proposed for its author; the person adds it again if they still
want it.

**Anomalies are attributed evidence.** Show every anomaly with an `against`. An unattributed
`UnconfirmedProposal` is only the chain still being confirmed — a member app may hide it, a
viewer should show it.

**A watchman is named by pubky in genesis and finds the chain from `index/active`.** It
needs credit (`free` or seconds) or it will not engage. Its Grant kid is minted on each
sign‑in and is not the keypair file, so a restart rotates the kid and keeps the pubky;
receipts are signed by the kid. One receipt decides nothing: with two engaged witnesses a
single outage leaves every time question asserted; three adjudicate through one. A minority
clock, however far off, does not decide. `receipt.consistent` is not a validity condition.

**Connections are scarce.** A browser allows about six to one host, and each live event
stream holds one. The client subscribes to the other members' folders, never mine, and
closes them when the seats change or the page leaves. A user action with no answer in twenty
seconds gives the form back and says so.

**The reader is `verifyFrom`, not a second client.** It has no seat and does not vote. It
polls while the chain is open and stops when it is final, including after a refresh. Home is
the index markers: `index/finished` before `index/active`, deduplicated by URL.

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
homeserver and need a Postgres (`docs/DOCKER.md` has the one‑liner).

## Not yet there

- Witness disagreement by more than `poll_ms` is specified (§11.3) and not yet compared in
  the fold. `AnomalyKind::Witness` is defined and not yet emitted (§11.6).
- No L402 or Paykit settlement in the watchman yet; `Operator::credit`/`free` is the ledger.

## Conventions

British English in prose and identifiers. No trailing whitespace. Doc comments cite the spec
section they implement. Prefer a property test or a flow test that would have caught the bug
over a unit test that asserts the fix.
