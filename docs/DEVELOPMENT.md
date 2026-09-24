# Development

```
cargo test --workspace                                          # core invariants + client and watchman flows
cargo test -p pubky-mayfly --test invariants                    # property tests alone
cargo test -p pubky-mayfly-client --test testnet -- --ignored   # against a real homeserver
cargo test -p pubky-mayfly-watchman --test testnet -- --ignored # the watchman on a real homeserver
cargo clippy -p pubky-mayfly -p pubky-mayfly-rules -p pubky-mayfly-client \
  --no-default-features --target wasm32-unknown-unknown         # the browser build stays green
```

Core, rules and the client compile for `wasm32-unknown-unknown` (`rustup target add
wasm32-unknown-unknown`). The client's Pubky SDK store and signer sit behind its default
`pubky-sdk` feature; the wasm build turns it off and JavaScript supplies the `Store` and
`Signer` instead (spec §16.2.1). CI (`.github/workflows/ci.yml`) runs both builds.

The client flows in `crates/client/tests/flows.rs` and the watchman flows in
`crates/watchman/tests/watchman.rs` run over an in-memory store on any machine. The same flows
in each crate's `tests/testnet.rs` run grant sessions against a `pubky-testnet`
`EphemeralTestnet`, which needs a Postgres for the homeserver. They are `#[ignore]`d for that
reason. The one-liner is in [Docker](DOCKER.md).

The rules the tests exist to protect are [Principles](PRINCIPLES.md).
