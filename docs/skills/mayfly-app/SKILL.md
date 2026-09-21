---
name: mayfly-app
description: Build applications and rules on Mayfly, the verifiable hashchain protocol among a small fixed set of parties on Pubky (crates pubky-mayfly, pubky-mayfly-rules, pubky-mayfly-client, pubky-mayfly-watchdog). Use when writing a Mayfly app (shared list, chess, agreed document), implementing a Rules module, driving ChainClient, engaging a watchdog, or testing chains over MemoryStore or a pubky-testnet.
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
| `pubky-mayfly-rules` | Rules modules: `list/1` today. Add new rules here. |
| `pubky-mayfly-client` | `ChainClient`: the app's whole surface. `Store`/`Signer` traits with `MemoryStore`/`LocalSigner` (tests) and `PubkyStore`/`SessionSigner` (Pubky SDK). |
| `pubky-mayfly-watchdog` | `Watchdog` (one chain) and `Operator` (many chains, customers, credit). Run it as a service; apps only *name* a watchdog in genesis. |
| `mayfly-demo` (`crates/demo`) | The worked example: a narrated shopping list on a testnet with a live explorer. Read `src/main.rs` for how an app drives `ChainClient` and a watchdog end to end, and `src/explorer.rs` for how a bystander reads and renders a chain. |

Full API cheat‑sheet: [reference.md](reference.md).

## The app loop

An app never reasons about rounds, votes or files. It does four things:

1. **Create or join.** Initiator: `ChainClient::create(rules, store, signer, GenesisSpec)`, then
   share `client.invite_url()`. Others: `ChainClient::open_url(rules, store, signer, &url)`,
   `sync()`, show the genesis (parties, quorum, witnesses, rules) to the user, then `join()`.
   Joining is consent to the fault model (§6.6) — never auto‑join.
2. **Act on every change.** `let actions = client.act().await?` whenever an event arrives
   (`wait_for_event` on `PubkyStore`, `wait_for_change` otherwise), on a timer, and after any
   user action. `act` syncs, mirrors, and does the honest choreography of §8.2 itself:
   confirms valid rules proposals, re‑proposes after a dead round, passes, skips a silent
   proposer after `think_ms`. It is idempotent and never double‑votes.
3. **Put decisions to the user.** `Action::Decision` is a `close`, `recover` or `witnesses`
   candidate: render it, then `confirm(hash)` or `reject(hash)` (or `repropose`/`pass` if
   `repropose: true`; `confirm_abandoned` for an abandoned close). `Action::MyTurn { round }`:
   propose something or `pass()`. `Action::AwaitingWitnesses`: show the watchdog's silence as
   the reason (§11.2). `Action::Rejected` means a link in my round was not a valid candidate
   and I refused it with evidence (§8.2 step 4) — show it; it needs no reply.
4. **Propose.** `client.propose_body(&Body::Add { .. })` for rules content. Protocol actions have
   their own methods: `propose_close(Agreed|Finished)`, `propose_abandoned(&[subject])` +
   `confirm_abandoned`, `propose_rekey(new_signer)`, `propose_recover(new_signer)`.

Read state with `client.state()?` (the rules' `State` at the head) and `client.verdict()`
(`status`, `committed`, `open`, `seats`, `engaged`, `anomalies`). Show `anomalies` — they are
attributed evidence, never something to hide.

## Rules that must survive into app code

- **Commitment consults votes only.** A watchdog receipt is never a validity condition. Never
  block an append on witnesses except through `Policy::await_witnesses`, which holds *my own*
  vote and is off by default.
- **Sync before voting.** Every vote follows a fresh fold over every reachable folder; `act`
  does this. Do not cache "the current round" across events.
- **A filename is never identity.** Records are matched by BLAKE3 of their bytes. Do not parse
  paths to decide anything; the fold does it.
- **Location is authorisation, signatures are proof.** Never trust a record because of whose
  folder it sat in.
- **Genesis names a witness by pubky only.** The watchdog finds the chain itself from the
  party's `index/active/<chain_id>` marker, which the client writes; no request API (§11.2).

## Writing a rules module

Implement `pubky_mayfly::rules::Rules` in `crates/rules/src/<name>.rs` with id `<name>/1`:

- `type Body`: an enum with `#[serde(tag = "kind", rename_all = "snake_case")]`. The tag
  becomes the link's `kind`; `propose_body` relies on it.
- `apply` is a **pure, deterministic** function of `(state, link)`; reject anything invalid
  with `RulesError`. The fold runs it on every party's machine; two honest verifiers must reach
  identical bytes from `canonical_state`, so keep state as a plain struct with fixed field
  order and no floats, maps with unstable order, or wall‑clock reads.
- `may_append` is eligibility (who may propose what now); `obliged` is whose clock runs
  (§10, §11.3). They are independent: in chess the side to move is both; in a list nobody is
  obliged.
- `close` turns an abandoned/agreed/finished close into an `Outcome`; `status` says when the
  state is terminal.
- `reference_hash` is pinned in genesis; a placeholder string is fine until release (§6.6).
- Set `options.time_control { think_ms, respond_ms }` in `GenesisSpec` if the rules care about
  time; otherwise both default to 24 h.

Test rules two ways: unit tests on `apply` alone, and a flow over `MemoryStore` driven by
`act()` (see `crates/client/tests/list.rs`).

## Testing pattern

```rust
let shared = MemoryStore::shared();
let identity = Keypair::random();
let signer = LocalSigner::mint(&identity, "myapp.example", NOW_S, lifetime_secs);
let store = MemoryStore::new(identity.public_key().z32(), Arc::clone(&shared));
// share one Arc<AtomicU64> clock via .with_clock(..) on every client and watchdog
```

Deterministic party choice: `pubky_mayfly::vote::designated(chain, seq, round, n)` says whose
round it is — pick "the silent one" from it rather than hoping. Run everything with
`cargo test --workspace`; the `tests/testnet.rs` files run the same flows against a real
homeserver and need a Postgres (README has the Docker one‑liner).

## Not yet there

- No JS/WASM bindings yet: the client uses `tokio` time and `Send` futures. A web app needs
  the SDK's `signJws` binding and a WASM‑safe `Store`. Prefer a Rust app first.
- Rules today: `list/1`. Chess needs `shakmaty` and PGN fixtures (§16.2 phase 4).
- No L402 or Paykit settlement in the watchdog yet; `Operator::credit`/`free` is the ledger.

## Conventions

British English in prose and identifiers. No trailing whitespace. Doc comments cite the spec
section they implement. Prefer a property test or a flow test that would have caught the bug
over a unit test that asserts the fix.
