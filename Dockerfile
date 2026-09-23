# syntax=docker/dockerfile:1
# The `mayfly-watchdog` service image (MAYFLY.md §16.2.1 E).
#
# Build from the pubky-homeserver checkout that contains mayfly/, not from mayfly/ itself: the
# workspace depends on ../pubky-common, ../pubky-sdk and ../pubky-testnet by path (README,
# "Layout"), so the context has to hold both trees.
#
#   docker build -f mayfly/Dockerfile -t mayfly-watchdog .
#
# BuildKit reads mayfly/Dockerfile.dockerignore for this Dockerfile, which keeps both
# workspaces' target/ directories and .git out of the context.

# ── Build ──────────────────────────────────────────────────────────────────────────────────
FROM rust:1-bookworm AS builder

# aws-lc-sys (rustls) and lmdb-master-sys (pkarr) compile C; aws-lc-sys wants cmake.
RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src

# The parent workspace's manifests, then only the crates the path dependencies name:
# pubky-testnet depends on pubky-homeserver and test_utils/pubky_test.
COPY Cargo.toml Cargo.lock ./
COPY pubky-common ./pubky-common
COPY pubky-sdk ./pubky-sdk
COPY pubky-homeserver ./pubky-homeserver
COPY pubky-testnet ./pubky-testnet
COPY test_utils ./test_utils
COPY mayfly ./mayfly

WORKDIR /src/mayfly
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/mayfly/target \
    cargo build --release --locked -p pubky-mayfly-watchdog --bin mayfly-watchdog \
    && cp target/release/mayfly-watchdog /mayfly-watchdog

# ── Run ────────────────────────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home --home-dir /var/lib/mayfly-watchdog \
       --shell /usr/sbin/nologin mayfly

COPY --from=builder /mayfly-watchdog /usr/local/bin/mayfly-watchdog

USER mayfly
WORKDIR /var/lib/mayfly-watchdog

# The identity file (`--keypair-file`, default /var/lib/mayfly-watchdog/keypair) lives here.
# Chains name this pubky in their genesis; a container without this volume mints a new
# identity every start and strands every engagement the old one held.
VOLUME /var/lib/mayfly-watchdog

# `--health-addr 0.0.0.0:8790` (MAYFLY_WATCHDOG_HEALTH_ADDR) serves /healthz and /status.
EXPOSE 8790

ENTRYPOINT ["mayfly-watchdog"]
