# Mayfly for JavaScript

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

## The example web apps

```
apps/list   the shared shopping list, for its members: sign in with Pubky Ring, create, invite,
            join, add / tick / untick / remove, archive, close
apps/view   the chain viewer, for anyone: paste a chain or record link, follow it live or read
            it finished, every link decoded, every verification failure on the record it belongs to
```

Both are Vite + React static sites over `crates/wasm` and the SDK's JS package, with a
`/testnet/` flavour (`npm run build:testnet`) for the local testnet. Each has its own README.
They take the SDK from `../pubky-sdk/bindings/js/pkg` (run its `npm run build` first) and the
module from `crates/wasm` (`npm run build` there) as `file:` dependencies until both are
published.

The shopping list is the only app in this repository. Further apps are their own projects.
