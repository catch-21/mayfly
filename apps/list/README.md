# Mayfly list

The shared shopping list, for the people on it (spec §15 item 1, §16.2.1 phase D). Every
change is a signed record on its author's homeserver, confirmed by every other member before
it counts; anyone with the link can replay the history. The read-only viewer for that is
`apps/view`.

```
npm install
npm run typecheck
npm run build             # mainnet flavour → dist/
npm run build:testnet     # local testnet flavour → dist/testnet/, served under /testnet/
npm run dev               # Vite dev server (mainnet flavour; VITE_TESTNET=true npm run dev for testnet)
```

Both `@synonymdev/pubky` and `@synonymdev/mayfly` are `file:` dependencies on the sibling
checkouts (`pubky-sdk/bindings/js/pkg`, built with its `npm run build`, and `crates/wasm`,
built with `npm run build`) until they are published: the SDK's `signJws` is on the fork
branch only.

## Configuration (build time)

| Variable | Meaning | Default |
| --- | --- | --- |
| `VITE_TESTNET` | `true` for the local testnet (pkarr relay on `localhost:15411`, homeserver `8pinxx…5ewo`, HTTP relay `localhost:15412`) | mainnet relays |
| `VITE_CLIENT_ID` | The app's client id; records live under `/pub/<client id>/mayfly/` | `list.mayfly.example` |
| `VITE_VIEWER_URL` | Where the viewer is deployed; adds "open in viewer" links | unset (no links) |

## What it does

- **Sign in** with Pubky Ring: a grant auth flow shown as a QR code. The grant's client key is
  what signs every record (§5). The testnet flavour also offers a one-click fresh identity on
  the local homeserver. Sessions persist in the SDK's browser store.
- **Your lists** come from your own homeserver: the `index/active/` and `index/finished/`
  markers the client writes (§7). Nothing is stored anywhere else.
- **Create** names the members by pubky (you first) and, optionally, a watchman; everyone must
  confirm every change. The result is an invite link, `pubky://…/mayfly/chains/<id>/`.
- **Join** shows the list's terms — members, unanimity, watchman — and signs your agreement
  only when you say so.
- **Add, tick, untick, remove** are proposals; the others' apps confirm them inside `act()`
  and the item appears as confirmed. A proposal not yet confirmed shows in amber with its
  vote count. Two members proposing at once resolves itself through a dead round (§6.4), with
  no user involvement.
- **Archive** stops changes and keeps the list; **close** ends the chain and needs every
  member's agreement, which arrives as a decision card in their app.
- Anything the verifier attributes to a member — a tampered mirror, an equivocation — is
  listed at the bottom, with the viewer for the evidence.

The loop is the one from `crates/client/tests/list.rs`: `act()` after every action, on a
three-second timer, and whenever a member's homeserver reports a change on its event stream.
