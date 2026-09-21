# Mayfly

A verifiable chain among a small set of parties, on Pubky. Every append is a signed,
hash-linked record on the author's own homeserver, committed by a quorum of the other parties'
confirmations within a voting round, and replayable by anyone from files alone. An optional
watchdog adds impartial time.

The design is `docs/MAYFLY.md`. It is at v0.8 after six independent reviews; the
implementation is guided by the invariants those reviews converged on, written down as property
tests before any transport code.

## Layout

```
crates/
  core/     pubky-mayfly          records, fold (verifier), rounds and votes, Grant checks,
                                     witness quorum, Rules trait, N-party simulator and the
                                     tally/1 test rules. WASM-safe, no I/O.
  rules/    pubky-mayfly-rules    list/1, later chess/1 and document/1
  client/   pubky-mayfly-client   ChainClient: sync-before-voting, propose/confirm/reject/
                                     mirror, recover, change watching; Store and Signer traits
                                     with in-memory and Pubky SDK implementations
  watchdog/ pubky-mayfly-watchdog Watchdog: engagement, one signed receipt per observed record
                                     in causal order, consistency flags, mirror tier; Operator:
                                     free or prepaid customers, engagement from index markers,
                                     renewal while credit lasts
  demo/     mayfly-demo           the narrated shopping list on a testnet, with a live explorer
docs/
  MAYFLY.md                       the specification
```

This directory is an independent git repository that happens to live inside a checkout of
`pubky-homeserver`, so that the `pubky-common`, `pubky` and `pubky-testnet` path dependencies
resolve during development. The SDK change in §16.3 (`GrantCredential::sign_jws`, `grant_jws`,
`client_public_key`) lives in that checkout's `pubky-sdk` until it is published; move this
directory out once it is.

## Working principles (from the spec)

- Commitment consults votes only: never death evidence, never receipts.
- Nothing waits for a third party: a witness receipt is never a validity condition.
- Every time verdict is provisional until the records it touches are final, and is reached by
  the witness quorum, never by one receipt.
- Embedded quorum certificates are history.

## Development

```
cargo test --workspace                                          # core invariants + client and watchdog flows
cargo test -p pubky-mayfly --test invariants                    # property tests alone
cargo test -p pubky-mayfly-client --test testnet -- --ignored   # against a real homeserver
cargo test -p pubky-mayfly-watchdog --test testnet -- --ignored # the watchdog on a real homeserver
```

The client flows in `crates/client/tests/flows.rs` and the watchdog flows in
`crates/watchdog/tests/watchdog.rs` run over an in-memory store on any machine. The same flows
in each crate's `tests/testnet.rs` run grant sessions against a `pubky-testnet`
`EphemeralTestnet`, which needs a Postgres for the homeserver. They are `#[ignore]`d for that
reason. Docker Postgres is enough, but the testnet's default URL uses your OS user with no
password, so set `TEST_PUBKY_CONNECTION_STRING` to the container's user and password:

```
docker run --name pubky-postgres -e POSTGRES_USER=postgres -e POSTGRES_PASSWORD=postgres \
  -p 127.0.0.1:5432:5432 -d postgres:18
TEST_PUBKY_CONNECTION_STRING='postgres://postgres:postgres@localhost:5432/postgres' \
  cargo test --workspace -- --ignored
```

## The demo

`crates/demo` is a narrated shopping list among three people on a local Pubky testnet, with a
watchdog. It walks the happy paths (create, invite, join, append, converge) and the sad ones
(a rule refusing a proposal, a forged record, competing proposals, a tampered mirror, a party
going quiet and being closed out with the watchdog adjudicating), pausing for Enter between
steps, and serves a live explorer page showing every homeserver's files and the verified chain.

```
TEST_PUBKY_CONNECTION_STRING='postgres://postgres:postgres@localhost:5432/postgres' \
  cargo run -p mayfly-demo                          # Docker Postgres on 5432 (user/password required)
cargo run -p mayfly-demo -- --auto --port 8787      # no pauses; explorer on another port
cargo run -p mayfly-demo -- --auto --pause-secs 7   # unattended, but lingering as a reader would
RUST_LOG=pubky=warn cargo run -p mayfly-demo        # show the SDK's transport decisions
```

Open `http://127.0.0.1:8787/` beside the terminal. The page reads the homeservers the way a
bystander would (public listings and `GET`s, then the verifier), so it shows what the files
prove rather than what the demo believes. Click any `.jws` file to decode it: header `typ`,
the payload with every embedded confirmation, receipt and Grant unpacked, the signature
checked under the key the record names, and the bytes checked against the `ETag` and the file
name. Records stay compact JWS on disk (§6), so a generic file browser shows them as opaque
strings; this panel is where they are read.

The testnet runs on the well-known ports (`StaticTestnet`: pkarr relay `15411`, homeserver
`6286`/`6287`/`6288`, DHT bootstrap `6881`), so the [Pubky explorer's testnet
mode](https://explorer.pubky.app/testnet/) can browse the same homeserver's raw files while the
demo runs. Stop anything else holding those ports first (another testnet, a Docker
`homeserver-testnet`).

## Building on Mayfly

`docs/skills/mayfly-app/SKILL.md` is the guide to writing an app or a rules module, for people
and agents alike, with an API cheat-sheet (`reference.md`) beside it; `AGENTS.md` points agents
there on entry. The shape of an app is
`crates/client/tests/list.rs`: join from an invite URL, call `act()` on every change, put
`Action::Decision`s to the user.

British English in prose and identifiers. No trailing whitespace.
