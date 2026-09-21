# Mayfly API reference (cheat‑sheet)

Section numbers refer to `docs/MAYFLY.md`.

## `pubky_mayfly_client::ChainClient<R: Rules, S: Store, K: Signer>`

| Method | Purpose |
| --- | --- |
| `create(rules, store, signer, GenesisSpec)` | Write genesis as `parties[0]`; writes `index/active/<id>`. |
| `open(rules, store, signer, chain, (owner, folder))` / `open_url(rules, store, signer, url)` | A client for an existing chain; reads nothing until `sync`. |
| `invite_url()` / `chain_url(&folder)` | `pubky://<owner>/pub/<client_id>/mayfly/chains/<id>/`. |
| `sync()` → `SyncReport { verdict, suspects, folders }` | Read every known folder, fold, discover declared folders and witnesses. Moves the marker to `index/finished/` once final. |
| `join()` | Confirm genesis with my Grant and folder (§8.1). |
| `act()` → `Vec<Action>` | Sync + mirror + honest step (§8.2). Idempotent. |
| `propose_body(&R::Body)` / `propose(kind, Value)` | Rules content at the open seq. Refused before writing if `may_append`/`apply` say no. |
| `confirm(hash)` / `reject(hash)` | Vote in my round on a candidate from `verdict().open.candidates`. |
| `repropose(hash)` / `pass()` / `skip()` | Designated‑round moves (§6.3, §6.4). `act` does these for rules content. |
| `lowest_voted_in_dead_round()` | What the designated proposer carries forward (§6.4). |
| `propose_close(Agreed\|Finished)` | Ordinary close; others confirm (via `Action::Decision`). |
| `propose_abandoned(&[party])` / `confirm_abandoned(hash)` | Close on a silent party (§6.8); provisional until a witness quorum adjudicates it. |
| `propose_reveal()` | Commit‑reveal nonce when rules `wants_reveals` (§6.6). |
| `propose_rekey(K)` / `propose_recover(K)` | Key change (§6.7). Recover is slow and vetoable by the old key. |
| `mirror()` | Copy committed links, QCs, receipts, engagements into my folder (§7). `act` does it. |
| `wait_for_change(timeout)` / `wait_for_event(timeout)` (PubkyStore) | Block until a folder changes (poll / SSE). |
| `state()` → `Option<R::State>` | Rules state at the head (`None` before rules initialise). |
| `verdict()` → `Option<&Verdict>` | The last fold. |
| `my_index()` / `signer()` / `store()` / `chain()` / `folders()` / `add_folder()` | Plumbing. |
| `policy: Policy { await_witnesses, poll, max_files_per_sync }` | Client policy, not protocol. |
| `with_clock(fn)` | Tests: control `ts` and skip timing. |

### `Action`

`Confirmed(h)`, `Reproposed { earlier, link }`, `Passed(h)`, `Skipped(h)`,
`Rejected { link, reason }`, `Decision { candidate, round, repropose }`, `MyTurn { round }`,
`AwaitingWitnesses { have, of, want }`.

### `GenesisSpec`

`new(parties)`, `.with_apps(&[client_id...])` (folder hints), fields `confirm_quorum: Option<u32>`
(None = unanimity), `witnesses: Vec<pubky>`, `recovery_delay_ms`, `max_body_bytes`,
`options: Value` (rules options; `time_control` is read by the protocol).

### Errors worth matching

`AlreadyVoted { seq, round }`, `NotDesignated { round, designated }`, `NoSuchCandidate`,
`RoundDead`, `AwaitingWitnesses { .. }`, `NotSeated`, `NoGenesis`, `NotOpen`, `Rules(msg)`.

## `pubky_mayfly::fold::Verdict`

| Field | Meaning |
| --- | --- |
| `committed: Vec<Committed { link, author, qc, is_final, witnessed: (m, k) }>` | The chain, seq 0 up. `is_final` once a successor embedded the QC. |
| `status: Status` | `Ongoing`, `Stalled { seq, round, dead, awaiting }`, `Paused { seq, close: CloseState }`, `Closed(Outcome)`, `Abandoned { subjects, outcome }`. |
| `open: Option<OpenView { seq, round, dead, candidates, voters }>` | The open seq; `candidates` are already fold‑valid. |
| `seats: Vec<Seat { pubky, kid, client_id, paths, grant_exp, commit }>` | Established parties. |
| `engaged: Vec<Engaged { pubky, kid, until, poll_ms, path }>` | Witnesses at the head. |
| `anomalies: Vec<Anomaly { against, seq, kind, evidence }>` | Attributed misbehaviour (§11.6). |
| `is_final()`, `head()`, `committed_hashes()`, `genesis()` | Helpers. |

Seatless verification: `pubky_mayfly_client::chain::verify_from(&rules, &store, &chain, (owner, folder))`.

## `Rules` trait (`pubky_mayfly::rules`)

```rust
type State: Clone + Serialize + DeserializeOwned;
type Body: Serialize + DeserializeOwned;           // #[serde(tag = "kind")]
fn id(&self) -> &'static str;                       // "list/1"
fn reference_hash(&self) -> &'static str;
fn init(&self, genesis, confirmations, nonces) -> Result<State, RulesError>;
fn wants_reveals(&self, genesis) -> bool;
fn obliged(&self, state) -> Vec<PartyIndex>;        // whose clock runs
fn may_append(&self, state, party, kind) -> bool;   // who may propose what
fn apply(&self, state, link) -> Result<State, RulesError>;  // pure
fn status(&self, state) -> Status;                  // Ongoing | Finished(Outcome)
fn close(&self, state, &CloseBody) -> Result<Outcome, RulesError>;
fn canonical_state(&self, state) -> Vec<u8>;        // hashed into every link
```

## Storage and signing

- `Store`: `me()`, `list(owner, prefix)`, `get(owner, path)`, `put(path, bytes)`, `delete(path)`.
  `MemoryStore::new(me, shared)` / `PubkyStore::new(pubky, session)` / `PubkyStore::read_only(pubky)`.
- `Signer`: `pubky()`, `kid()`, `client_id()`, `grant_jws()`, `path()`, `sign_jws(typ, value)`.
  `LocalSigner::mint(&identity, client_id, now_s, lifetime_s)` / `SessionSigner::from_session(&session)`
  (needs a Grant session; uses `GrantCredential::sign_jws` from the SDK fork).
- Layout (`pubky_mayfly_client::layout`): `Folder::for_app(client_id)` → `/pub/<client_id>/mayfly/`;
  `.chain(&id)` → `links/`, `confirms/`, `rejects/`, `receipts/<kid>/`; `.witness(&id)`;
  `.active(&id)` / `.finished(&id)`; `parse_chain_url(url)`.

## Watchdog (`pubky_mayfly_watchdog`)

- `Watchdog::new(store, signer, chain, initiator, Terms)` → `engage()`, `poll()`, `extend(until)`,
  `run_until(stop)`, `receipts()`, `observed_at(&hash)`.
- `Terms::receipts(until)` / `Terms::mirror(until)` / `.poll_every(ms)`.
- `Operator::new(store, signer, terms)` → `free(pubky)`, `credit(pubky, secs)`, `engage_for(secs)`,
  `renew_before(secs)`, `sweep()` → `SweepReport { engaged, extended, lapsed, declined, receipts }`.
  Reads customers' `/pub/` for `index/active/<id>` markers; charges free customer → initiator →
  first by pubky.

## Spec map

| Topic | Section |
| --- | --- |
| Parties, keys, Grants | §5 |
| Records and JSON rules | §6 (link 6.1, confirm 6.2, reject 6.3, rounds 6.4, genesis 6.6, key change 6.7, close 6.8) |
| Storage layout | §7 |
| Flows | §8 (create 8.1, append 8.2, competing 8.3, finishing 8.4) |
| Verification | §9 (algorithm 9.2) |
| Rules interface | §10 |
| Watchdogs | §11 (engagement/credit 11.2, receipts/stopwatch 11.3, misbehaviour 11.6) |
| Security analysis, residuals | §12 |
| Plan and status | §16 |
