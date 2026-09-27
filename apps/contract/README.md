# Mayfly contract

Two parties agreeing a text. One proposes it; the other accepts, rejects it outright, or
sends a revision. Accept or reject ends the negotiation. A finished close records that
ending. While an offer is open, either party may propose to walk away; refusing that close
withholds consent and the offer stays open. If that close then comes back as "put it
forward again, or let it go", **Let it go** ends the negotiation for both parties. It
does not pass the question on. A finished close, once the negotiation has already ended,
is only recorded — there is no way to pass that question along.

```
npm install
npm run typecheck
npm test
npm run build             # mainnet flavour → dist/
npm run build:testnet     # local testnet flavour → dist/testnet/, served under /testnet/
npm run dev               # Vite dev server (mainnet flavour; VITE_TESTNET=true npm run dev for testnet)
```

`@synonymdev/pubky`, `@synonymdev/mayfly` and `@synonymdev/mayfly-browser` are `file:`
dependencies on the sibling checkouts (`pubky-sdk/bindings/js/pkg`, `crates/wasm` and
`js/browser`, each built with its `npm run build`) until they are published.

The app is thin on purpose. `src/rules.ts` is `contract/1`. `src/useContract.ts` names the
verbs over the browser client's `useChainSession`. The screens render each phase. Sign-in,
the loop, decisions, held proposals, and the home index are the client's.

## Configuration (build time)

| Variable | Meaning | Default |
| --- | --- | --- |
| `VITE_TESTNET` | `true` for the local testnet | mainnet relays |
| `VITE_CLIENT_ID` | The app's client id; records live under `/pub/<client id>/mayfly/` | `contract.mayfly.example` |

A chain with these rules is read in this app. The list viewer only ships `list/1`.
