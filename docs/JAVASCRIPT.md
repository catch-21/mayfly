# Mayfly for JavaScript

Two packages. A web app depends on the second; the first is underneath it.

## The wasm module: `@synonymdev/mayfly` (`crates/wasm`)

The client, the verifier and the plain-data views (`crates/client/src/view.rs`) as one
wasm-bindgen module. The page supplies storage and signing as objects — built from an SDK
`Pubky` and grant `Session` by `js/pubky-glue.js` — so `@synonymdev/pubky` stays the only
network code and no session object crosses between the two wasm modules (spec §16.2.1).

Rules are a shipped id (`"list/1"`) or an object the app writes in JavaScript, so an app
with its own rules needs no wasm rebuild (`crates/wasm/src/jsrules.rs`).

```
cd crates/wasm
npm run build     # wasm-pack → pkg/
npm test          # the shared-list flow, a JavaScript rules module, and error shapes
```

`crates/wasm/README.md` shows the API. The npm name is provisional.

## The browser client: `@synonymdev/mayfly-browser` (`js/browser`)

What a web app drives. Thin on purpose: the protocol logic — the `act()` loop, held
proposals, decisions, where a member stands, the home index, the reader — is the Rust client's
and reaches the page through the wasm module. This package adds what a browser has: sign in
with Pubky Ring, event streams from the other members, a timer, a timeout, and what a person
is told. React hooks under `/react`. Bring your own rules and screens.

```
cd js/browser
npm run build     # tsc → dist/
npm test          # two members and a reader over an in-memory store, in Node
```

`js/browser/README.md` is the guide; `docs/skills/mayfly-app/SKILL.md` says what the client
does for a page and why.

## The example web app

```
apps/list   the shared shopping list, for its members: sign in with Pubky Ring, create, invite,
            join, add / edit / tick / untick / remove, archive, close
apps/view   the chain viewer, for anyone: paste a chain or record link, follow it live or read
            it finished, every link decoded, every verification failure on the record it belongs to
```

Both are Vite + React static sites over the browser client, with a `/testnet/` flavour
(`npm run build:testnet`) for the local testnet. Each has its own README. They take the SDK
from `../pubky-sdk/bindings/js/pkg` (run its `npm run build` first), the module from
`crates/wasm` and the client from `js/browser` (`npm run build` in each) as `file:`
dependencies until they are published.

The shopping list is the only app in this repository. Further apps are their own projects.
