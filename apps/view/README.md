# Mayfly chain viewer

A read-only web page that follows a Mayfly chain from a `pubky://` link. No sign-in. It uses
`useChainReader` from `@synonymdev/mayfly-browser`: the reader lists every folder the chain
declares (the initiator's, every seat's, every engaged witness's), runs the same verifier the
parties run, and this page shows each committed link with its decoded body, the open seq, the
rules state, attributed anomalies, and a row per file with the checks on its bytes: signature,
bytes against the homeserver ETag, bytes against the hash in the file name. A failing check is
highlighted on the file it belongs to.

Spec: `docs/MAYFLY.md` §14 (chain explorer) and §16.2.1 phase G.

## What a link looks like

Any of these open the same chain:

```
pubky://<owner>/pub/<client_id>/mayfly/chains/<CHAIN_ID>/
pubky://<owner>/pub/<client_id>/mayfly/chains/<CHAIN_ID>/links/00000000-<h16>.jws
pubky://<owner>/pub/<client_id>/mayfly/chains/<CHAIN_ID>/confirms/00000003-<h16>-<kid>.jws
```

`<owner>` is the initiator's pubky (z32), `<client_id>` the app that holds their seat, and
`<CHAIN_ID>` 26 Crockford Base32 characters. A record URL is cut back to the chain folder. The
link is kept in the page hash (`#pubky://...`) so a viewer URL can be shared; the viewer also
opens whatever chain is in the hash on load.

## Build

Dependencies are `file:` paths on the sibling checkouts, until the packages are published.
Build the wasm module first (`npm run build` in `crates/wasm`), then the browser client
(`npm run build` in `js/browser`):

- `@synonymdev/pubky` from `pubky-sdk/bindings/js/pkg` (the SDK; embeds its own wasm)
- `@synonymdev/mayfly` from `mayfly/crates/wasm`
- `@synonymdev/mayfly-browser` from `mayfly/js/browser`

```sh
npm install
npm run typecheck        # tsc --noEmit
npm run build            # mainnet flavour -> dist/
npm run build:testnet    # testnet flavour -> dist/testnet/, base /testnet/
npm run dev              # vite dev server
```

### Flavours

- **Mainnet** (`npm run build`): `new Pubky()`, the SDK's public relays.
- **Testnet** (`npm run build:testnet`): sets `VITE_TESTNET=true`, so the page uses
  `Pubky.testnet()` and talks to a local testnet (pkarr relay on `localhost:15411`, the
  homeserver the testnet publishes). Start one with `cargo run -p pubky-testnet` or the
  `docker-compose.yml` in the repository root.

### Base path and GitHub Pages

The build is static. `npm run build` writes the mainnet flavour to `dist/` with base `/`;
`npm run build:testnet` writes the testnet flavour to `dist/testnet/` with base `/testnet/`,
so publishing `dist/` gives both, as pubky-explorer does. For a GitHub Pages project site
served under `/<repo>/`, pass the base explicitly:

```sh
npx vite build --base /<repo>/
VITE_TESTNET=true npx vite build --base /<repo>/testnet/ --outDir dist/testnet
```

and publish `dist/`. The page fetches `assets/mayfly_bg-<hash>.wasm` relative to `base`, so
the base must match the path the site is served under. Nothing else is configured: no router,
no server, no environment at run time.

## How it reads a chain

`src/mayfly.ts` builds a `MayflyApp` with a read-only store and a `RulesRegistry` of the
rules this build can run: `list/1` and `chess/1`, shipped in the wasm module, and `contract/1`
from the contract app. A chain whose rules are not in the registry is still listed. The page
shows the parties, the witnesses genesis names, and each link's body and signature check,
and says that this is not a verified chain.

`parseChainUrl` cuts a record link back to the chain folder, and that URL is what
`useChainReader` follows. The reader walks the folders, decodes each `.jws`, and verifies
when it knows the rules. It polls every 4 s while the chain is open and stops once the
verifier says it is final, including after a refresh. One reader per page, so a later poll
refetches only files whose content hash changed.

## Layout

```
src/main.tsx            React root
src/App.tsx             landing, link form, the reader, page composition
src/mayfly.ts           MayflyApp, the read-only store, the rules registry
src/marks.ts            which rows go red (own check failed) or amber (anomaly evidence)
src/types.ts            re-exports the browser client's views, plus the list/1 state it draws
src/format.ts           shortening, local time, error text
src/components/         Header, Timeline, Files, RecordPanel, RulesState, Anomalies, Json, Short
src/styles.css          the stylesheet; dark, no framework
```
