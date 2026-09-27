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
| `act()` → `Vec<Action>` | Sync + mirror + honest step (§8.2), then the oldest held proposal, then `auto_pass`. Idempotent. |
| `hold(&body)` / `hold_value(json)` / `held()` / `withdraw(i)` / `clear_held()` | Proposals kept until a round takes them (§6.4); `act` sends them. |
| `phase()` → `Phase` | `Loading`, `Stranger`, `Invited`, `Waiting`, `Open`, `Ended` (§8.1). |
| `session_view()` → `SessionView` | `phase`, named `parties`, `my_index`, `pending` (live round only), `held`, `url`. |
| `named_parties()` | From the committed genesis, or genesis as written before commit. |
| `propose_body(&R::Body)` / `propose(kind, Value)` | Rules content at the open seq. Refused before writing if `may_append`/`apply` say no. |
| `confirm(hash)` / `reject(hash)` | Vote in my round on a candidate from `verdict().open.candidates`. |
| `repropose(hash)` / `pass()` / `skip()` | Designated‑round moves (§6.3, §6.4). `act` does these for rules content. |
| `lowest_voted_in_dead_round()` | What the designated proposer carries forward (§6.4). |
| `propose_close(Agreed\|Finished)` | Ordinary close; others confirm (via `Action::Decision`). |
| `propose_abandoned(&[party])` / `confirm_abandoned(hash)` | Close on a silent party (§6.8); provisional until a witness quorum adjudicates it. |
| `propose_reveal()` | Commit‑reveal nonce when rules `wants_reveals` (§6.6). |
| `propose_rekey(K)` / `propose_recover(K)` | Key change (§6.7). Recover is slow and vetoable by the old key. |
| `mirror()` | Copy committed links, QCs, receipts, engagements into my folder (§7). `act` does it. |
| `wait_for_change(timeout)` | Poll until a known folder changes. On a `PubkyStore`, prefer the store's own `wait_for_event(timeout)` (the homeserver's event stream). |
| `state()` → `Option<R::State>` | Rules state at the head (`None` until genesis commits). |
| `verdict()` → `Option<&Verdict>` | The last fold. |
| `arrangement()` → `Option<Arrangement>` | Genesis terms before commit: `parties`, `rules`, `witnesses`, `confirm_quorum`. |
| `my_index()` / `signer()` / `store()` / `chain()` / `folders()` / `add_folder()` | Plumbing. |
| `policy: Policy { await_witnesses, poll, max_files_per_sync, auto_pass }` | Client policy, not protocol. `auto_pass` is off by default. |
| `with_clock(fn)` | Tests: control `ts` and skip timing. |

### `Action`

`Confirmed(h)`, `Reproposed { earlier, link }`, `Passed(h)`, `Skipped(h)`,
`Rejected { link, reason }`, `Decision { candidate, round, repropose }`, `MyTurn { round }`,
`Proposed(h)`, `HeldRefused { body, reason }`, `AwaitingWitnesses { have, of, want }`.

### Free functions

`my_chains(&store, folder)` → `Vec<ChainMarker { url, finished }>` from the index markers
(§7). `reader::Reader::new().load(&store, url, |id| rules_for(id))` → `Loaded` (`rules`,
`rules_known`, `view`, `view_error`, `folders`, `files` with decoded `RecordView`s); the
reader caches decoded records by content hash. `reader::normalise_chain_url` cuts a record
link back to its chain. `Error::name()`, `Error::is_transient()`.

### `GenesisSpec`

`new(parties)`, `.with_apps(&[client_id...])` (folder hints), fields `confirm_quorum: Option<u32>`
(None = unanimity), `witnesses: Vec<pubky>`, `recovery_delay_ms`, `max_body_bytes`,
`options: Value` (rules options; `time_control` is read by the protocol).

### Errors worth matching

`AlreadyVoted { seq, round }`, `NotDesignated { round, designated }`, `NoSuchCandidate`,
`RoundDead`, `AwaitingWitnesses { .. }`, `NotSeated`, `NoGenesis`, `NotOpen`, `Rules(msg)`,
`Oversize { len, max }` (record larger than genesis `max_body_bytes`; nothing was written).
`AlreadyVoted`, `RoundDead`, `NotDesignated` and `AwaitingWitnesses` are retryable. JS errors
use the variant name (`Oversize`, `AlreadyVoted`, …), plus `Busy` and `InvalidInput`.

## `pubky_mayfly::fold::Verdict`

| Field | Meaning |
| --- | --- |
| `committed: Vec<Committed { link, author, qc, is_final, witnessed: (m, k) }>` | The chain, seq 0 up. `is_final` once a successor embedded the QC. |
| `status: Status` | `Ongoing`, `Stalled { seq, round, dead, awaiting }`, `Paused { seq, close: CloseState }`, `Closed(Outcome)`, `Abandoned { subjects, outcome }`. |
| `open: Option<OpenView { seq, round, dead, candidates, voters }>` | The open seq; `candidates` are already fold‑valid. While `dead`, `round` is the dead round; `Action::Decision.round` is the next one. |
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
- Views (`pubky_mayfly_client::view`): `chain_view(&rules, &verdict, &suspects)` → `ChainView`
  (`parties` from the *committed* genesis — empty until everyone has joined; use
  `Arrangement` before that — `status`, `is_final`, committed `LinkView`s with body and
  base64url hash, `open`, `seats`, `engaged`, `anomalies`, `suspects`, rules `state` as JSON);
  `ActionView::from(&action)`;
  `decode_record(bytes, name, etag)` → `RecordView` (typ, payload with embedded JWSs unpacked,
  `signature_ok`, `hash_matches_etag`, `hash_matches_name`). All `Serialize`; what a page renders.

## Browser client (`js/browser`, `@synonymdev/mayfly-browser`)

- `new MayflyApp({ clientId, testnet?, wasm? })` → `startRingSignIn()`, `qrDataUrl(flow)`,
  `testnetSignUp(token?)`, `remember(s)`, `restore()`, `forget(s?)`, `party(session)` →
  `{ me, store, signer }`, `readOnlyStore()`, `isPubky(s)`, `folder`.
- `new ChainSession({ rules, store, signer, url, wake?, tickMs?, actionTimeoutMs?, autoPass?,
  holdProposals?, clock? })` → `start()`, `stop()`, `subscribe(fn)`, `state: ChainState`
  (`phase`, `view`, `arrangement`, `parties`, `myIndex`, `state`, `pending`, `decisions` as
  `{ action, seenAt }` with the hash at `action.candidate.hash`, `held`, `myTurn`,
  `awaitingWitnesses`, `busy`, `error` as a `describeError` string, `lastActions`); `join()`,
  `propose(body, { hold? })`, `withdraw(body)`, `confirm(h)`, `reject(h)`, `repropose(h)`,
  `pass()`, `proposeClose("agreed"|"finished")`, `proposeAbandoned([i])`,
  `confirmAbandoned(h)`, `refresh()`. `pubkyWake(app.pubky)` is the live `wake`. The state
  is the module's `session()` and `view()` plus what the browser owns (busy, error, streams).
- `new ChainReader({ store, registry, url, pollMs? })` → `start()`, `stop()`, `refresh()`,
  `state: { loaded, error, busy, final }`; `loadChain(store, registry, url)` → `Loaded`
  (`url`, `rules`, `rules_known`, `view`, `view_error`, `folders`, `files`).
- `myChains(store, folder)`, `createChain(rules, store, signer, spec)` → invite URL,
  `parsePubkys(raw, { exclude? })`, `isPubky(s)`.
- `RulesRegistry(shippedRules(), [modules])` → `resolve(id)`, `has(id)`, `ids()`;
  `RulesModule` is the JavaScript rules shape; `RulesRef = string | RulesModule`.
- `loadMayfly(wasm?)`, `shippedRules()`, `parseChainUrl(url)` → `ChainRef` (record links
  accepted), `isChainUrl`, `verifyFrom(rules, store, url)`, `decodeRecord(bytes, name, etag?)`,
  `describeError(e)`, `isTransient(e)` (reads `e.transient`), `errorName(e)`,
  `hashBytes(bytes)` (BLAKE3, unpadded base64url, for bytes an app names itself).
- React (`@synonymdev/mayfly-browser/react`): `useSession(app)`, `useRingSignIn(app,
  onSession)`, `useMyChains(app, party)`, `useCreateChain(party, rules)`,
  `useChainSession(app, party, url, rules, options?)`, `useChainReader(options)`.
- `@synonymdev/mayfly-browser/core` is everything above that does not need the Pubky SDK;
  the Node tests run over it with a memory store.

## Wasm module (`crates/wasm`, `@synonymdev/mayfly`)

- `await init()`; then `ChainClient.create(rules, store, signer, spec)` /
  `ChainClient.openUrl(rules, store, signer, url)` with `store` and `signer` as plain objects
  (`js/pubky-glue.js`: `storeFromPubky(pubky, session?)`, `signerFromSession(session)`).
  `rules` is a shipped id (`"list/1"`) or a `RulesModule` object.
- Methods mirror the Rust client in camelCase: `sync()` → `ChainView`, `join()`, `act()` →
  `ActionView[]` (`kind: "confirmed" | "decision" | "my_turn" | "proposed" | "held_refused"
  | …`), `proposeBody(body)`, `hold(body)`, `withdraw(i)`, `clearHeld()`, `confirm(hash)`,
  `reject`, `repropose`, `pass`, `skip`, `proposeClose("agreed"|"finished")`,
  `proposeAbandoned([i])`, `confirmAbandoned`, `mirror()`, `waitForChange(ms)`,
  `setClock(fn)`, `setPolicy({ awaitWitnesses?, pollMs?, maxFilesPerSync?, autoPass? })`.
  Hashes are base64url strings. Async calls on one client are queued and run one at a time.
  `view()`, `session()` (`{ phase, parties, my_index, pending, held, url, me }`), `state()`,
  `arrangement()`, `myIndex()` are synchronous and read a snapshot taken after the last call.
- `new ChainReader(store, (id) => rules | undefined)` → `load(url)` → `Loaded`; caches decoded
  records by content hash across loads.
- Free functions: `verifyFrom(rules, store, chainUrl)` → `ChainView`; `myChains(store,
  folder)`; `decodeRecord(bytes, name, etag?)` → `RecordView`; `hashBytes(bytes)` →
  unpadded base64url BLAKE3; `parseChainUrl` (record links accepted; returns `url`),
  `chainUrl`, `isPubky`, `designatedProposer`, `rulesIds` (the shipped ids only).
- `KeyedSigner(clientId)`: a self-contained signer for Node and tests.
- Errors are JS `Error`s with `name` = the client error variant, plus `Busy` and
  `InvalidInput`; `transient: true` on the ones that mean "not yet".

## Watchman (`pubky_mayfly_watchman`)

- `Watchman::new(store, signer, chain, initiator, Terms)` → `resume()` (take back the
  engagement and receipts on file; never receipt twice), `engage()`, `poll()` (by name, cheap;
  safe to cancel), `audit()` (with content hashes; catches a rewritten mirror),
  `note_changed(owner, path)`, `extend(until)`, `run_until(stop)`, `receipts()`,
  `observed_at(&hash)`, `is_engaged()`.
- `Terms::receipts(until)` / `Terms::mirror(until)` / `.poll_every(ms)`.
- `Operator::new(store, signer, terms)` → `free(pubky)`, `credit(pubky, secs)`, `engage_for(secs)`,
  `renew_before(secs)`, `concurrency(n)`, `deadline(d)`, `audit_every(n)`, `sweep()` →
  `SweepReport { engaged, resumed, extended, lapsed, declined, receipts, timed_out, failed,
  slow_customers, audited }`, `poll_chain(&id)`, `mark_changed(&id, owner, path)`,
  `chains()`, `folders_of(&id)`. Polls engaged chains first, `concurrency` at a time under
  `deadline`; reads customers' `/pub/` for `index/active/<id>` markers; charges free customer →
  initiator → first by pubky. Resumes rather than re-engages when its folder holds a live
  engagement.
- `Store::list_names(owner, prefix)`: a listing without content hashes, one request per page;
  what the watchman polls with. `Store::list` adds a `HEAD` per file for the tamper check.
- `mayfly-watchman` (binary, `crates/watchman`): an `Operator` over a Pubky grant session as a
  service — `--network`, `--homeserver`, `--free`, `--credit <pubky>=<secs>`, `--keypair-file`,
  `--health-addr` (`/healthz`, `/status`), `--deadline-secs`, `--concurrency`,
  `--audit-every`, `--events`; `MAYFLY_WATCHMAN_*` env or `--config` TOML. Follows the
  parties' event streams and polls a chain when its folders change; sweeps as the fallback.
  Name its pubky in `GenesisSpec::witnesses`. `docs/WATCHMAN.md`.

## Spec map

| Topic | Section |
| --- | --- |
| Parties, keys, Grants | §5 |
| Records and JSON rules | §6 (link 6.1, confirm 6.2, reject 6.3, rounds 6.4, genesis 6.6, key change 6.7, close 6.8) |
| Storage layout | §7 |
| Flows | §8 (create 8.1, append 8.2, competing 8.3, finishing 8.4) |
| Verification | §9 (algorithm 9.2) |
| Rules interface | §10 |
| Watchmen | §11 (engagement/credit 11.2, receipts/stopwatch 11.3, misbehaviour 11.6) |
| Security analysis, residuals | §12 |
| Plan and status | §16 |
