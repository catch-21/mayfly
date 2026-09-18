//! Error type shared across the crate.

use crate::hash::Hash;

/// Anything that stops a record being a candidate or a vote, or stops a fold.
///
/// Note the distinction the specification draws everywhere: an `Error` here means "this record
/// or this input is not usable"; it is never how misbehaviour is reported. Misbehaviour is an
/// [`crate::fold::Anomaly`] attributed to a key, and the fold carries on.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// JWS could not be split, decoded, or its signature did not verify.
    #[error("jws: {0}")]
    Jws(String),

    /// Header `typ` is not one this crate accepts (§5.2).
    #[error("unexpected typ {0:?}")]
    Typ(String),

    /// Payload violated the strict parsing rules of §6.
    #[error("payload: {0}")]
    Payload(String),

    /// A hash string was not 32 bytes in one of the three accepted encodings (§6).
    #[error("hash: {0}")]
    Hash(String),

    /// Grant did not verify, or did not bind what the record claims (§5.2).
    #[error("grant: {0}")]
    Grant(String),

    /// Genesis failed a safety-parameter check (§6.6); the chain never existed.
    #[error("genesis invalid: {0}")]
    Genesis(String),

    /// The record's `chain` did not match the chain being folded.
    #[error("record belongs to chain {found}, expected {expected}")]
    WrongChain {
        /// The chain the record names.
        found: String,
        /// The chain being folded.
        expected: String,
    },

    /// Two committed links at one seq, or two committed successors with different `prev`
    /// (§9.2 step 3d): the parties collectively equivocated and the fold stops.
    #[error("collective equivocation at seq {seq}: {a} and {b}")]
    CollectiveEquivocation {
        /// Sequence number at which it happened.
        seq: u64,
        /// One committed link.
        a: Hash,
        /// The other.
        b: Hash,
    },

    /// A rules implementation refused or failed.
    #[error("rules: {0}")]
    Rules(String),

    /// Not yet implemented in this skeleton.
    #[error("not implemented: {0}")]
    Unimplemented(&'static str),
}
