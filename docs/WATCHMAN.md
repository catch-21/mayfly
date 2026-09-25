# The watchman service

`mayfly-watchman` (in `crates/watchman`) is the hosted watchman of spec §16.2.1 E: one
identity signed in to one homeserver as one app, running an `Operator` (§11.2). Its customers
are pubkys, watched for free or against prepaid watch-time; it finds their chains from the
`index/active/<chain_id>` markers their clients write anyway, engages if genesis names it,
receipts every record it observes, renews before `until` while credit lasts, and lets an
engagement lapse when the marker moves to `index/finished/`. The service stops only on SIGINT
or SIGTERM.

**How it keeps up.** Two things drive it. It follows each party's homeserver event stream
for the folders of the chains it watches, and polls a chain within a second of one of its
folders changing. Every `--sweep-secs` it also sweeps everything: polls every watched chain,
reads the customers' markers, renews what is due. The sweep is the fallback the stream does
not need to be reliable for; a stream that cannot be opened costs promptness, nothing else,
and is opened again at the next sweep. Polls list folders by name — one request per folder
prefix, however long the chain — and fetch only what has not been read. Every
`--audit-every` sweeps the listing asks for content hashes too, so a mirror overwritten in
place is caught (§7) even where no event named it; an event on a known path does the same at
once.

**Slow homeservers.** Chains are polled `--concurrency` at a time, each under
`--deadline-secs`. A homeserver that does not answer in time delays only the chain it is
part of, which is reported (`timed_out` on `/status`) and tried next sweep with nothing lost:
a poll marks nothing done until its receipt is on file. Engaged chains are served before
discovery; a customer whose `/pub/` will not list in time is skipped (`slow_customers`) and
looked at next sweep. A sweep that fails outright is logged and tried again next interval.

**Restarts.** On start the watchman reads its own `witness/<chain_id>/` folders back: every
receipt it has issued, and the engagement on file. It receipts nothing twice, so the
`observed_at` first written stands, and it never publishes an engagement ending earlier than
the one on file. A restart signs in afresh and so gets a new client key (`kid`); a new
engagement is published under it, ending no earlier than the old one and charged only for
time beyond it, and the receipts under the old key remain that witness's receipts — the fold
reads a witness as its pubky across every key it has held (`engage/<kid>.jws` keeps each
engagement so they stay verifiable).

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
| `--deadline-secs` | `MAYFLY_WATCHMAN_DEADLINE_SECS` | the sweep interval |
| `--concurrency` | `MAYFLY_WATCHMAN_CONCURRENCY` | `8` |
| `--audit-every` (sweeps; `0` never) | `MAYFLY_WATCHMAN_AUDIT_EVERY` | `20` |
| `--events true\|false` | `MAYFLY_WATCHMAN_EVENTS` | `true` |
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
`resumed`, `extended`, `lapsed`, `declined`, `receipts`, `timed_out`, `failed`,
`slow_customers`, `audited`), `last_error`, `event_polls` and `event_receipts` (polls made
because a stream reported a change, and what they wrote), and `streams` (event streams open).

The container image and the local compose stack are in [Docker](DOCKER.md).
