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
                                     witness quorum, Rules trait. WASM-safe, no I/O.
  rules/    pubky-mayfly-rules    list/1, later chess/1 and document/1
  client/   pubky-mayfly-client   storage layout, propose/confirm/reject/mirror, SSE sync
docs/
  MAYFLY.md                       the specification
```

This directory is an independent git repository that happens to live inside a checkout of
`pubky-homeserver`, so that the `pubky-common` and `pubky` path dependencies resolve during
development. Move it out once those are published with the SDK change in §16.3.

## Working principles (from the spec)

- Commitment consults votes only: never death evidence, never receipts.
- Nothing waits for a third party: a witness receipt is never a validity condition.
- Every time verdict is provisional until the records it touches are final, and is reached by
  the witness quorum, never by one receipt.
- Embedded quorum certificates are history.

## Development

```
cargo test --workspace
cargo test -p pubky-mayfly --test invariants   # property tests
```

British English in prose and identifiers. No trailing whitespace.
