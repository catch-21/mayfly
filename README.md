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
`EphemeralTestnet`, which needs a Postgres for the homeserver: a local server on the default
port, `TEST_PUBKY_CONNECTION_STRING`, or `pubky-testnet`'s `docker-postgres` feature. They are
`#[ignore]`d for that reason. With Docker:

```
docker run --name pubky-postgres -e POSTGRES_USER=postgres -e POSTGRES_PASSWORD=postgres \
  -p 5433:5432 -d postgres:18
TEST_PUBKY_CONNECTION_STRING='postgres://postgres:postgres@localhost:5433/postgres' \
  cargo test --workspace -- --ignored
```

## Building on Mayfly

`docs/skills/mayfly-app/SKILL.md` is the guide to writing an app or a rules module, for people
and agents alike, with an API cheat-sheet (`reference.md`) beside it; `AGENTS.md` points agents
there on entry. The shape of an app is
`crates/client/tests/list.rs`: join from an invite URL, call `act()` on every change, put
`Action::Decision`s to the user.

British English in prose and identifiers. No trailing whitespace.
