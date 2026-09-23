# Mayfly

A verifiable chain among a small set of parties, on Pubky. Every append is a signed,
hash-linked record on the author's own homeserver, committed by a quorum of the other parties'
confirmations within a voting round, and replayable by anyone from files alone. An optional
watchman adds impartial time.

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
  watchman/ pubky-mayfly-watchman Watchman: engagement, one signed receipt per observed record
                                     in causal order, consistency flags, mirror tier; Operator:
                                     free or prepaid customers, engagement from index markers,
                                     renewal while credit lasts; the mayfly-watchman service
                                     binary
  demo/     mayfly-demo           the narrated shopping list on a testnet, with a live explorer
  wasm/     pubky-mayfly-wasm     the client, verifier and views for JavaScript, over a store
                                     and signer the page supplies (npm, name provisional)
docs/
  MAYFLY.md                       the specification
Dockerfile                        the mayfly-watchman image (build from the parent checkout)
docker-compose.yml                Postgres, a testnet and the watchman, end to end
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
watchman. It walks the happy paths (create, invite, join, append, converge) and the sad ones
(a rule refusing a proposal, a forged record, competing proposals, a tampered mirror, a party
going quiet and being closed out with the watchman adjudicating), pausing for Enter between
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

## The watchman service

`mayfly-watchman` (in `crates/watchman`) is the hosted watchman of spec §16.2.1 E: one
identity signed in to one homeserver as one app, running an `Operator` (§11.2) on a timer.
Its customers are pubkys, watched for free or against prepaid watch-time; it finds their chains
from the `index/active/<chain_id>` markers their clients write anyway, engages if genesis names
it, receipts every record it observes, renews before `until` while credit lasts, and lets an
engagement lapse when the marker moves to `index/finished/`. A sweep that fails is logged and
tried again next interval; the service stops only on SIGINT or SIGTERM.

Every flag has a `MAYFLY_WATCHMAN_*` environment variable, and `--config <file>` names a TOML
file with the same keys in snake case; a flag or variable wins over the file, the file over
the default. `--homeserver` is the only required setting.

```
cargo run -p pubky-mayfly-watchman --bin mayfly-watchman -- \
  --network testnet \
  --homeserver 8pinxxgqs41n4aididenw5apqp1urfmzdztr8jt4abrkdn435ewo \
  --keypair-file ./watchman.key \
  --free <alice pubky> --free <bob pubky> \
  --credit <carol pubky>=604800 \
  --health-addr 127.0.0.1:8790
```

| Flag | Environment variable | Default |
| --- | --- | --- |
| `--network mainnet\|testnet\|testnet:<host>` | `MAYFLY_WATCHMAN_NETWORK` | `mainnet` |
| `--homeserver <pubky>` | `MAYFLY_WATCHMAN_HOMESERVER` | required |
| `--signup-token <token>` | `MAYFLY_WATCHMAN_SIGNUP_TOKEN` | none |
| `--client-id <id>` | `MAYFLY_WATCHMAN_CLIENT_ID` | `watchman.mayfly.example` |
| `--keypair-file <path>` | `MAYFLY_WATCHMAN_KEYPAIR_FILE` | `/var/lib/mayfly-watchman/keypair` |
| `--free <pubky>` (repeatable) | `MAYFLY_WATCHMAN_FREE` (comma-separated) | none |
| `--credit <pubky>=<secs>` (repeatable) | `MAYFLY_WATCHMAN_CREDIT` (comma-separated) | none |
| `--engage-secs` / `--renew-before-secs` | `..._ENGAGE_SECS` / `..._RENEW_BEFORE_SECS` | `86400` / `3600` |
| `--poll-ms` / `--sweep-secs` | `..._POLL_MS` / `..._SWEEP_SECS` | `5000` / `15` |
| `--tier receipts\|mirror` | `MAYFLY_WATCHMAN_TIER` | `receipts` |
| `--health-addr <ip:port>` | `MAYFLY_WATCHMAN_HEALTH_ADDR` | off |
| `--config <path>` | `MAYFLY_WATCHMAN_CONFIG` | none |

`--client-id` decides the folder everything is published under, `/pub/<client_id>/mayfly/`;
change it and verifiers looking for the engagement under the old folder no longer find it.
Logging is `tracing`; `RUST_LOG` is respected and defaults to `info`.

**The keypair.** On first start the service generates a keypair, writes the 32 secret bytes as
hex to `--keypair-file` (mode `0600`, parent directories created), and logs the resulting pubky
loudly; afterwards it loads the file. That pubky is what chains name in their genesis
(`GenesisSpec::witnesses`) and what every engagement and receipt is verified against, so the
file has to outlive the process and the container: lose it and every engagement it holds is
stranded, and every chain that named it has to seat a new witness with a `witnesses` link
(§11.2). Back it up like any signing key.

**Signing in.** The service signs in first, which is the ordinary restart. If that fails —
no account yet, or a testnet whose DHT has forgotten the `_pubky` record — it signs up with
`--signup-token` if given; a homeserver that already has the account answers `409 Conflict`,
in which case only the record is republished. Then it signs in. Start-up tries this a dozen
times, five seconds apart, so a homeserver still coming up does not fail the service.

**Health.** With `--health-addr`, `GET /healthz` is `200` while the last sweep succeeded within
three sweep intervals and `503` otherwise (including before the first sweep), and `GET /status`
is JSON: `pubky`, `kid`, `client_id`, `network`, `homeserver`, `path`, `healthy`, `watching`
(`chain`, `until`, `receipts` per engaged chain), `customers` (`pubky`, `credit` as `"free"`
or `{"seconds": n}`), `sweeps`, `errors`, `last_sweep_at`, `last_sweep` (`engaged`,
`extended`, `lapsed`, `declined`, `receipts`) and `last_error`.

**Docker.** The image is built from the *parent* checkout, because the workspace depends on
`../pubky-common`, `../pubky-sdk` and `../pubky-testnet` by path (see "Layout"):

```
cd ..                                                   # the pubky-homeserver checkout
docker build -f mayfly/Dockerfile -t mayfly-watchman .
docker volume create mayfly-watchman
docker run -d --name mayfly-watchman --restart unless-stopped \
  -v mayfly-watchman:/var/lib/mayfly-watchman -p 127.0.0.1:8790:8790 \
  -e MAYFLY_WATCHMAN_HOMESERVER=<homeserver pubky> \
  -e MAYFLY_WATCHMAN_SIGNUP_TOKEN=<token from the homeserver's admin> \
  -e MAYFLY_WATCHMAN_FREE=<pubky>,<pubky> \
  -e MAYFLY_WATCHMAN_HEALTH_ADDR=0.0.0.0:8790 \
  mayfly-watchman
docker logs mayfly-watchman | head                      # the pubky to name in genesis
```

The image runs as the non-root user `mayfly`, declares `/var/lib/mayfly-watchman` as a volume
(the keypair; see above) and exposes `8790`. `docker-compose.yml` here is the local end to
end: `postgres:18`, the testnet built from the parent `Dockerfile` with `BUILD_TARGET=testnet`
on the well-known ports, and the watchman against it, keypair in a named volume, health on
`http://127.0.0.1:8790/`:

```
MAYFLY_WATCHMAN_FREE=<alice pubky>,<bob pubky> docker compose up --build
```

The testnet's homeserver advertises its endpoints in its own pkarr record as `127.0.0.1` and
`localhost`, and the SDK's `testnet:<host>` form only moves the DHT bootstrap node and the
pkarr relay to `<host>`, not the homeserver, so a sibling container cannot reach it. The
compose file therefore runs the watchman in the testnet container's network namespace
(`network_mode: service:testnet`) with the plain `testnet` network form; the file's comments
give the alternative for a testnet on another host.

## Mayfly for JavaScript

`crates/wasm` is the client, the verifier and the plain-data views (`crates/client/src/view.rs`)
as one wasm-bindgen module. The page supplies storage and signing as objects — built from an
SDK `Pubky` and grant `Session` by `js/pubky-glue.js` — so `@synonymdev/pubky` stays the only
network code and no session object crosses between the two wasm modules (spec §16.2.1).

```
cd crates/wasm
npm run build     # wasm-pack → pkg/
npm test          # the shared-list flow of crates/client/tests/list.rs, through the module
```

`crates/wasm/README.md` shows the API. The npm name is provisional.

## Building on Mayfly

`docs/skills/mayfly-app/SKILL.md` is the guide to writing an app or a rules module, for people
and agents alike, with an API cheat-sheet (`reference.md`) beside it; `AGENTS.md` points agents
there on entry. The shape of an app is
`crates/client/tests/list.rs`: join from an invite URL, call `act()` on every change, put
`Action::Decision`s to the user. In JavaScript the same shape is `crates/wasm/tests/list.test.mjs`.

British English in prose and identifiers. No trailing whitespace.
