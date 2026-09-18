//! Mayfly client (§7, §8): the I/O half.
//!
//! The core crate decides what bytes mean; this crate moves bytes. It owns the storage layout
//! under `/pub/<client_id>/mayfly/`, the propose / confirm / reject / mirror flows of §8, the
//! "sync before voting" merge of every reachable source, SSE watching with a polling fallback,
//! and watchdog engagement.
//!
//! Phase 0 dependency (§16.3): `GrantCredential::sign_jws(typ, claims)` in `pubky-sdk`, so a
//! browser or local-grant session can sign chain records with its Grant `cnf` key. Until it
//! lands, [`layout`] and the pure parts here are usable and the signing path is a `todo!`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod layout;

/// Client-side policy that is *not* protocol (§11.2): how many engaged witnesses must have
/// receipted the head before this client will confirm the next link. Default `0`. Under
/// unanimity a non-zero value holds the chain and is charged to this party as silence.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// `await_witnesses`; the default of `0` never holds a vote.
    pub await_witnesses: usize,
}
