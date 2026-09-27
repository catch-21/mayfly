# Mayfly chess

Two players, one legal move at a time. Shakmaty decides whether a move is legal. The other player's client confirms it only after that check. A watchman, named when the game is created, records how long each move took. Without one, the moves are ordered and not timed.

```
npm install
npm run typecheck
npm run build             # mainnet flavour → dist/
npm run build:testnet     # local testnet flavour → dist/testnet/, served under /testnet/
npm run dev               # Vite dev server (mainnet flavour; VITE_TESTNET=true npm run dev for testnet)
```

`@synonymdev/pubky`, `@synonymdev/mayfly` and `@synonymdev/mayfly-browser` are `file:`
dependencies on the sibling checkouts (`pubky-sdk/bindings/js/pkg`, `crates/wasm` and
`js/browser`, each built with its `npm run build`) until they are published.

The rules are `chess/1`, shipped in the wasm module. The page proposes a move, a resignation or a draw, and renders the board. It does not confirm a move itself.

## Configuration (build time)

| Variable | Meaning | Default |
| --- | --- | --- |
| `VITE_TESTNET` | `true` for the local testnet | mainnet relays |
| `VITE_CLIENT_ID` | The app's client id; records live under `/pub/<client id>/mayfly/` | `chess.mayfly.example` |
| `VITE_VIEWER_URL` | Where the chain viewer is deployed; adds "open in viewer" links | unset (no links) |

A chain with these rules is read in this app, board and all. The chain viewer shows the same watchman times on every link, and does not draw a board.
