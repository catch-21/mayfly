# Working in this repository

Mayfly is a verifiable hashchain protocol among a small, fixed set of parties on Pubky. The
specification is `docs/MAYFLY.md`; code cites it by section number, and so should you.

Before writing an app, a rules module, or anything that drives `ChainClient`, read
`docs/skills/mayfly-app/SKILL.md` — the guide for people and agents — and keep
`docs/skills/mayfly-app/reference.md`, the API cheat-sheet, to hand. The shape of an app is
`crates/client/tests/list.rs`.

Conventions: British English in prose and identifiers; no trailing whitespace; no empty lines
containing spaces; run `cargo fmt --all`, `cargo clippy --workspace --all-targets` and
`cargo test --workspace` before finishing. Testnet tests are `#[ignore]`d and need a Postgres
(see `README.md`).
