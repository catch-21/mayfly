//! Rules implementations (§10).
//!
//! `list/1` first: any member may append; kinds `add`, `edit`, `tick`, `untick`, `remove`,
//! `archive`. Trivial by design — it exercises multi-party confirmation, competing proposals
//! (and so rounds) and the explorer with no rules complexity. Further apps live in their own
//! projects and bring their own rules.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod list;
