# Property tests

A working list. Current coverage records the property tests that exist. Planned coverage sets
out the tests a review of the gaps suggested, in the order to work through them. Each planned
item says the property, the generator, and where the test should live. The specification is
[MAYFLY.md](MAYFLY.md).

`proptest` is a dependency of `pubky-mayfly` only. The property tests live in
`crates/core/tests/invariants.rs` and in the test module of `crates/core/src/encoding.rs`. Run
them all with `cargo test -p pubky-mayfly`.

## Current coverage

### Rounds, rotation, and witness quorum

These run against `vote` and `witness` directly, with no simulation, so they are fast and the
generators are small.

| Test | Pins | What it asserts |
| --- | --- | --- |
| `two_qcs_in_one_round_need_double_votes` | §6.4 | Over random rounds of 2..=6 parties, two quorum certificates in one round imply at least `2q − N` equivocators. |
| `empty_round_death_needs_rejects` | §6.4 | A round with no proposal dies only when `N − q + 1` parties vote for nothing. |
| `a_qc_round_is_alive` | §6.4 | A round holding a quorum certificate and no equivocation is never dead. |
| `rotation_covers_every_party` | §6.4 | Over `N` consecutive rounds, `designated` names every party exactly once, and is deterministic. |
| `one_receipt_decides_nothing` | §11.2 | `adjudicate` returns `Yes` or `No` only on a clean quorum; a single receipt never decides. |

`round_strategy` builds a round of `n` parties, each casting 0..=2 votes drawn from a pool of
one to three links or "nothing". Two votes from one party is deliberate, so equivocation is
reachable.

### The fold against honest simulations

These play a random honest run through `sim::Sim` and compare `fold::verify` with the
simulator's own model, then with itself under altered inputs. `run_strategy` draws 2..=4
parties, a quorum above half, one to five sequences, and per-party confirm-or-reject choices.

| Test | Cases | Pins | What it asserts |
| --- | --- | --- | --- |
| `commitment_is_independent_of_receipts_and_death_evidence` | 48 | §6.4, §9.2 step 3d | Deleting any subset of receipts and rejects never changes `committed[]` or the status. Anomalies that appear are only `UnjustifiedRound`, `PrematureSkip`, or `InvalidSkip`. |
| `honest_parties_converge_within_two_rounds` | 48 | §6.4 | Every sequence commits within two rounds, the fold agrees with the simulator, there are no anomalies, and only the head is provisional. |
| `any_byte_mutation_fails` | 48 | §6 | Flipping any one byte of any record makes that record fail verification, and never changes what is committed. |
| `close_verdicts_never_contradict_when_final` | 32 | §6.8 | With every receipt an abandoned close is adjudicated one way; any subset of receipts yields that same final verdict or a provisional one, and the committed prefix is unchanged. |
| `skips_are_bounded` | 32 | §6.3, §6.4, §11.3 | One party walking away under `q = N − 1` produces fewer than `N` skips per sequence, a second skip by the same party is `InvalidSkip`, and an immediate skip is `PrematureSkip` only when a witness can see it. |
| `the_verdict_does_not_depend_on_input_order` | 48 | §9.2 | Shuffling every input bucket independently, and reversing them all, leaves the verdict unchanged: committed links with their authors, finality, witnessed counts and times; status; the open seq; seats; engaged witnesses; recoveries; and the set of anomalies. A QC's members, the anomaly list, and an anomaly's evidence are compared as sets. |

### The fold against adversarial runs

These start from the same honest runs and then misbehave. Forgeries are written after the run
with the simulator's `forge_confirmation` and `forge_reject`; witness behaviours are set before
genesis. Each forgery names a real link, so it lands on a real `(seq, round)`, and its flags
move it off that spot or give it a wrong `state`. Six kinds are drawn: a confirmation by a
party's current key with or without a round and with the right or a wrong state; a reject
naming the link or nothing, in the link's round or the next; a reject under a key nobody
established; and a rally, where every party confirms one link, aimed at an uncommitted
competitor at the head when there is one. Together they reach `Equivocation`, `Obstruction`,
`RulesDivergence`, `InvalidSkip`, `LateVote`, `UnjustifiedRound` and `HostileSource`, and the
rally reaches `CollectiveEquivocation`.

| Test | Cases | Pins | What it asserts |
| --- | --- | --- | --- |
| `forged_votes_never_rewrite_history` | 48 | §6.4, §9.2 step 3d | After one to six forgeries the fold either verifies, with every link that was final still committed at its seq and the chain still open, or stops with `CollectiveEquivocation` at a seq that was not final. It never panics and never returns any other error, and the outcome is the same after every input bucket is shuffled. |
| `misbehaving_witnesses_change_nothing_but_witnessed_counts` | 48 | §11.2 | Under any mix of honest, dark, selective, skewed (up to three days either way) and receipt-deleting witnesses, `committed[]` matches the simulator, the status is ongoing, every anomaly is a witness anomaly, and *witnessed m/k* has `k` engaged and counts no dark witness. The verdict is unchanged under shuffling. |
| `verifiers_on_different_files_agree_on_history` | 48 | §9.1, §9.2 | Two verifiers each see the forged run with a random subset of links, confirmations, rejects and receipts removed. Each verifies or stops with `CollectiveEquivocation` or `NoChain`; neither calls the chain closed; and at every seq both hold final they hold the same link. |

Writing `forged_votes_never_rewrite_history` showed that the order of an `Anomaly`'s
`evidence` follows input order. Nothing reads evidence as a sequence, so the snapshot compares
it as a set; making the fold sort it would be a small change if a deterministic verdict is ever
wanted byte for byte.

### The strict parser

These are in the test module of `crates/core/src/encoding.rs`. The generator is a small JSON
grammar: documents up to three levels deep whose strings are drawn from a seven-character
alphabet, so repeated keys are common, and are spelled with a random mix of raw characters,
short escapes (`\"`, `\\`, `\/`) and `\u` escapes in either case, with a surrogate pair for the
character outside the BMP.

| Test | Pins | What it asserts |
| --- | --- | --- |
| `parsing_never_panics` | §6 | `parse_strict` refuses arbitrary bytes without panicking. |
| `parsing_near_json_never_panics` | §6 | The same over text drawn from JSON's own characters, so the scanner's bracket, string and escape branches are reached. |
| `duplicate_keys_are_rejected_however_spelled` | §6 "Parsing rules" | A document is rejected exactly when some object repeats a key, comparing keys as the strings they decode to. Two spellings of an accepted document, one with each object's members permuted, both parse to the value the document denotes. |
| `numbers_outside_the_safe_integers_are_rejected` | §6 | At any depth, an integer is accepted exactly when it is at most `2^53 − 1`; negative, fractional and exponent forms are refused. |

Writing `duplicate_keys_are_rejected_however_spelled` found and fixed a live defect: the scanner
compared raw literals, so `{"a":1,"\u0061":2}` was accepted and read as `{"a":2}`. Keys are now
unescaped before comparison, and `a_duplicate_key_is_a_duplicate_however_it_is_spelled` pins
the cases.

### What is covered by example, not by property

`invariants.rs` also holds hand-written scenarios: a dark witness, a late vote revealed after a
successor commits, mirrored quorum certificates, a missing or ambiguous genesis, the recover
veto window, the recovery delay under a witness quorum, a minority clock, folders following
declared paths, presence decided from files, obstruction and its exemptions, the
`max_body_bytes` cap, reveals seating the rules, witness rotation, and Grant windows.

Outside that file every test is an example. `rules/src/chess.rs` has eight fixtures (scholar's
mate, illegal moves, castling, promotion, stalemate, the fifty-move rule, threefold repetition,
resign and draw offers). `rules/src/list.rs` has two. `hash.rs` has five spelling cases and
`layout.rs` three path cases. `client/tests/flows.rs` has five scripted sessions, and the
watchman crate has scenario tests for engagement, receipts, and the operator's credit sweep.

## Planned coverage

Ordered by risk. Done and recorded above: the fold under any input order and the strict parser,
which guard the property the design rests on — every honest verifier, reading the same bytes,
reaches the same verdict — and the adversarial runs, which guard its safety story. What remains
is below.

### 1. Garbage records folded beside an honest run change nothing

`any_byte_mutation_fails` flips one byte inside an otherwise valid record. Folders are writable
by their owners and a verifier reads every declared path, so the realistic case is a wholly
invented file: random bytes, a well-formed JWS with a junk payload, a correctly signed record
of the wrong `typ`, or one from a different chain.

- **Property.** Injecting such records into an honest run's `Inputs` leaves `committed[]` and
  the status unchanged, adds only anomalies of an expected kind, and never panics. Separately,
  `Signed::decode` never panics on arbitrary bytes.
- **Generator.** `run_strategy` plus a strategy per kind of garbage.
- **Where.** `crates/core/tests/invariants.rs` for the fold half; `crates/core/src/record/mod.rs`
  for the decode half.

### 2. Chess rules agree with shakmaty on random games

`rules/src/chess.rs` is a thousand lines with eight hand-played fixtures, and it has an oracle
in-process: every legality question is shakmaty's. A wrong turn or a wrong terminal result
changes what may commit, so these are consensus rules, not display rules.

- **Properties**, each over random playouts from random legal positions:
  - every move in `chess_view().legal_uci` applies, and a random string that is not one of them
    is refused without changing the state;
  - the game ends on the move shakmaty itself calls checkmate, stalemate, threefold, or
    fifty-move, with the same result;
  - the obliged party alternates with the side to move;
  - a move declines an open draw offer, and the party who offered can never accept it;
  - a `san` is accepted exactly when it equals the SAN shakmaty computes for that move.
- **Where.** `crates/rules/src/chess.rs`. This adds `proptest` to `pubky-mayfly-rules`.

### 3. The client never decides what the fold would not

`client/src/chain.rs` promises that every action runs the fold first and then acts on its open
sequence. Five scripted sessions in `client/tests/flows.rs` cover it. Note that
[MAYFLY.md](MAYFLY.md) §16.2.1 says `act()` is property-tested in Rust; only the rounds and
skips underneath it are. This item is the one that makes that sentence true.

- **Property.** Random interleaved sequences of propose, confirm, reject, pass, close, and
  recover by several clients over the in-memory store: after each step the folders verify, and
  after a sync every client holds the same committed chain. No client writes a record the fold
  would reject.
- **Where.** `crates/client/tests/`. This is the largest item; it needs `proptest` on
  `pubky-mayfly-client` and a driver around the in-memory store.

### 4. Small round-trip properties

Low risk, and each is a few lines once the dependency is in place.

- **Hash spellings** (`crates/core/src/hash.rs`). For random 32-byte values, every spelling
  (`to_base64url`, `to_etag`, `to_content_hash`) parses back to the same bytes; a spelling with
  any character changed fails closed; `Hash::parse` and `ChainId::parse` never panic on
  arbitrary strings.
- **The watchman operator** (`crates/watchman/src/operator.rs`). Over random sweeps, markers,
  and credit balances: a receipt's `observed_at` for one record never goes backwards, the
  operator never engages a chain whose genesis does not name it or whose customer has no
  credit, and it never extends `until` past the credit remaining.
- **Chain URLs** (`crates/client/src/layout.rs`, `reader.rs`). `parse_chain_url` and
  `normalise_chain_url` round-trip every URL the layout builders emit, and neither panics on an
  arbitrary string.
