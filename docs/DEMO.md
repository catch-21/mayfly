# The demo

`crates/demo` is a narrated shopping list among three people on a local Pubky testnet, with a
watchman. It walks the happy paths (create, invite, join, append, converge) and the sad ones
(a rule refusing a proposal, a forged record, competing proposals, a tampered mirror, a party
going quiet and being closed out with the watchman adjudicating), pausing for Enter between
steps, and serves a live explorer page showing every homeserver's files and the verified chain.

It needs the Postgres from [Docker](DOCKER.md).

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
