# Docker

## Postgres for the ignored tests and the demo

Docker Postgres is enough for `tests/testnet.rs` and for [the demo](DEMO.md). The testnet's
default URL uses your OS user with no password, so set `TEST_PUBKY_CONNECTION_STRING` to the
container's user and password:

```
docker run --name pubky-postgres -e POSTGRES_USER=postgres -e POSTGRES_PASSWORD=postgres \
  -p 127.0.0.1:5432:5432 -d postgres:18
TEST_PUBKY_CONNECTION_STRING='postgres://postgres:postgres@localhost:5432/postgres' \
  cargo test --workspace -- --ignored
```

## The watchman image

The image is built from the *parent* checkout, because the workspace depends on
`../pubky-common`, `../pubky-sdk` and `../pubky-testnet` by path:

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
(the keypair; see [the watchman service](WATCHMAN.md)) and exposes `8790`.

## The local stack

`docker-compose.yml` is the local end to end: `postgres:18`, the testnet built from the parent
`Dockerfile` with `BUILD_TARGET=testnet` on the well-known ports, and the watchman against it,
keypair in a named volume, health on `http://127.0.0.1:8790/`:

```
MAYFLY_WATCHMAN_FREE=<alice pubky>,<bob pubky> docker compose up --build
```

The testnet's homeserver advertises its endpoints in its own pkarr record as `127.0.0.1` and
`localhost`, and the SDK's `testnet:<host>` form only moves the DHT bootstrap node and the
pkarr relay to `<host>`, not the homeserver, so a sibling container cannot reach it. The
compose file therefore runs the watchman in the testnet container's network namespace
(`network_mode: service:testnet`) with the plain `testnet` network form; the file's comments
give the alternative for a testnet on another host.
