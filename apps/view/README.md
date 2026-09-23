# Mayfly chain viewer

A read-only web page that follows a Mayfly chain from a `pubky://` link. No sign-in. It lists
every folder the chain declares (the initiator's, every seat's, every engaged witness's), runs
the same verifier the parties run (`verifyFrom` from `@synonymdev/mayfly`), and shows each
committed link with its decoded body, the open seq, the rules state, attributed anomalies, and a
row per file with the checks on its bytes: signature, bytes against the homeserver ETag, bytes
against the hash in the file name. A failing check is highlighted on the file it belongs to.

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

Dependencies are the two locally built wasm packages, referenced with `file:` paths:

- `@synonymdev/pubky` from `pubky-sdk/bindings/js/pkg` (the SDK; embeds its own wasm)
- `@synonymdev/mayfly` from `mayfly/crates/wasm` (run `npm run build` there first)

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

1. `parseChainUrl` on the (normalised) link gives `{ chain, owner, folder }`.
2. The initiator's chain folder is listed: `store.list(owner, folder + "chains/" + chain + "/")`.
   Every `.jws` is fetched and passed to `decodeRecord(bytes, path, etag)`.
3. Rules detection: the `links/00000000-<h16>.jws` file is genesis; its `payload.body.rules`
   is the rules id. If `rulesIds()` includes it, `verifyFrom(rules, store, chainUrl)` runs.
   Otherwise the page says which rules it lacks and still shows the decoded files.
4. Folder walk: from the verified head, every `seats[].paths` entry is listed under the seat's
   pubky (`<path>chains/<chain>/`) and every `engaged[].path` under the witness's pubky
   (`<path>witness/<chain>/`). Duplicates of the initiator folder are skipped.
5. While `is_final` is false the whole load runs again every 4 s (one in flight at a time);
   decoded records are cached by ETag so unchanged files are not refetched. Once final,
   polling stops.

## Layout

```
src/main.tsx            React root
src/App.tsx             landing, link form, live polling, page composition
src/mayfly.ts           wasm init, store, normaliseChainUrl, loadChain
src/marks.ts            which rows go red (own check failed) or amber (anomaly evidence)
src/types.ts            ChainView, LinkView, RecordView and friends (snake_case, as in Rust)
src/format.ts           shortening, local time, error text
src/components/         Header, Timeline, Files, RecordPanel, RulesState, Anomalies, Json, Short
src/styles.css          the stylesheet; dark, no framework
```
