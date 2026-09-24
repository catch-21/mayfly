# The watchman service

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

The container image and the local compose stack are in [Docker](DOCKER.md).
