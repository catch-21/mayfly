# Mayfly

<p align="center">
  <img src="docs/mayfly-logo.png" alt="Mayfly" width="360">
</p>

A verifiable chain among a small set of parties, on Pubky. Every append is a signed,
hash-linked record on the author's own homeserver, committed by a quorum of the other parties'
confirmations within a voting round, and replayable by anyone from the files alone. An optional
watchman adds impartial time.

The design is `[docs/MAYFLY.md](docs/MAYFLY.md)`.

## Start developing

This directory is its own git repository, nested inside a `pubky-homeserver` checkout so the
`pubky-common`, `pubky` and `pubky-testnet` path dependencies resolve. Until it is published,
the SDK change the client needs (`GrantCredential::sign_jws`, spec §16.3) lives in that
checkout's `pubky-sdk`.

```
cargo test --workspace
```

That is the whole check: the core invariants, and the client and watchman flows, on an
in-memory store. Nothing else has to be running.

The shopping list in `[apps/list](apps/list)` is the example application, and
`[apps/view](apps/view)` is the viewer anyone can open on the same chain. Each has its own
README. To watch a chain being built, run [the demo](docs/DEMO.md).

## Layout


| Path                     | What it is                                       |
| ------------------------ | ------------------------------------------------ |
| `crates/core`            | Records, the verifier, rounds and votes. No I/O. |
| `crates/rules`           | `list/1`, the shopping-list rules.               |
| `crates/client`          | `ChainClient`: what an application drives.       |
| `crates/watchman`        | The watchman, and the `mayfly-watchman` service. |
| `crates/demo`            | A narrated shopping list on a local testnet.     |
| `crates/wasm`            | The client and verifier for JavaScript.          |
| `js/browser`             | The browser client a web app drives.             |
| `apps/list`, `apps/view` | The example web app and its viewer.              |


Further applications are their own projects. A web app depends on `js/browser`; a Rust app on
the crates. They are not added under `apps/`.

## Further reading

- [Principles](docs/PRINCIPLES.md) — the Mayfly way.
- [Why every record is a JWS](docs/JWS.md) — a record proves its signer without the homeserver.
- [Development](docs/DEVELOPMENT.md) — the wasm target, clippy, and tests against a homeserver.
- [The demo](docs/DEMO.md) — the narrated list, and the live explorer.
- [The watchman service](docs/WATCHMAN.md) — flags, the keypair, health.
- [Docker](docs/DOCKER.md) — Postgres, the watchman image, the local stack.
- [Scaling](docs/SCALING.md) — what a watchman costs per sweep, where the limits sit, and what to change.
- [JavaScript](docs/JAVASCRIPT.md) — the wasm package, the browser client, and the example web apps.



## Building on Mayfly

`[docs/skills/mayfly-app/SKILL.md](docs/skills/mayfly-app/SKILL.md)` is the guide to writing an
application or a rules module, for people and agents, with the API cheat-sheet
`[reference.md](docs/skills/mayfly-app/reference.md)` beside it. `AGENTS.md` points agents
there on entry.

The shopping list is the only example application in this repository. The shape of an app is
`crates/client/tests/list.rs`: join from an invite URL, call `act()` on every change, and put
`Action::Decision`s to the user. In the browser that loop is `@synonymdev/mayfly-browser`
(`js/browser`); a web app supplies its rules and its screens and nothing else.