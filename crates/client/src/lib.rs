//! Mayfly client (§7, §8): the I/O half.
//!
//! The core crate decides what bytes mean; this crate moves bytes. It owns the storage layout
//! under `/pub/<client_id>/mayfly/`, the propose / confirm / reject / mirror flows of §8, the
//! "sync before voting" merge of every reachable source, change watching with a polling
//! fallback, and — later — watchman engagement.
//!
//! Everything is written against two small traits so the same flows run over an in-memory
//! store in tests and over the Pubky SDK in an app:
//!
//! - [`Store`]: read anyone's `/pub/` folder, write my own ([`MemoryStore`], `PubkyStore`);
//! - [`Signer`]: the party's identity, chain key and app, and a JWS signature under that key
//!   ([`LocalSigner`], `SessionSigner` over a Pubky grant session — §16.3).
//!
//! The SDK-backed pair lives behind the default `pubky-sdk` feature. Without it the crate
//! compiles for `wasm32-unknown-unknown`, where a JavaScript object implements the two traits
//! (§16.2.1); see [`portable`] for how the `Send` bounds differ by target.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod chain;
pub mod layout;
pub mod portable;
#[cfg(feature = "pubky-sdk")]
pub mod pubky_store;
pub mod reader;
pub mod signer;
pub mod store;
pub mod time;
pub mod view;

pub use chain::{
    my_chains, Action, ChainClient, ChainMarker, GenesisSpec, Phase, Policy, SyncReport,
};
#[cfg(feature = "pubky-sdk")]
pub use pubky_store::{PubkyStore, SessionSigner};
pub use signer::{LocalSigner, Signer};
pub use store::{Listed, MemoryStore, Store};

/// Anything that stops the client.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Storage failed.
    #[error("store: {0}")]
    Store(String),
    /// Signing failed, or the signer was misused.
    #[error("signer: {0}")]
    Signer(String),
    /// The core refused something.
    #[error(transparent)]
    Core(#[from] pubky_mayfly::Error),
    /// The rules refused something.
    #[error("rules: {0}")]
    Rules(String),
    /// No sync has happened yet, or the chain is closed.
    #[error("the chain has no open seq")]
    NotOpen,
    /// The genesis link has not been found in any known folder.
    #[error("no genesis found yet")]
    NoGenesis,
    /// This signer holds no seat in the chain.
    #[error("not a party to this chain")]
    NotSeated,
    /// §6.4 discipline: one vote per round.
    #[error("already voted in round {round} of seq {seq}")]
    AlreadyVoted {
        /// The seq.
        seq: u64,
        /// The round.
        round: u32,
    },
    /// §6.4: rounds `>= 1` have one designated proposer.
    #[error("round {round} is party {designated}'s to propose")]
    NotDesignated {
        /// The round.
        round: u32,
        /// Whose it is.
        designated: usize,
    },
    /// The hash names nothing valid at the open seq.
    #[error("no such candidate at the open seq")]
    NoSuchCandidate,
    /// The candidate's round is over; wait for the designated re-proposal.
    #[error("that proposal's round is dead")]
    RoundDead,
    /// Client policy (§11.2): the head is witnessed by fewer than `await_witnesses`.
    #[error("head witnessed {have}/{of}; waiting for {want}")]
    AwaitingWitnesses {
        /// Receipts held.
        have: usize,
        /// Engaged witnesses.
        of: usize,
        /// Policy.
        want: usize,
    },
    /// The action is not allowed in this state (message says why).
    #[error("{0}")]
    State(String),
    /// The signed record is larger than the chain's `max_body_bytes` (§6.6).
    #[error("record is {len} bytes; this chain allows {max}")]
    Oversize {
        /// The record's length.
        len: usize,
        /// Genesis `max_body_bytes`.
        max: u64,
    },
}

impl Error {
    /// The variant's name, as an app matches on it (`e.name` in JavaScript).
    pub fn name(&self) -> &'static str {
        match self {
            Error::Store(_) => "Store",
            Error::Signer(_) => "Signer",
            Error::Core(_) => "Core",
            Error::Rules(_) => "Rules",
            Error::NotOpen => "NotOpen",
            Error::NoGenesis => "NoGenesis",
            Error::NotSeated => "NotSeated",
            Error::AlreadyVoted { .. } => "AlreadyVoted",
            Error::NotDesignated { .. } => "NotDesignated",
            Error::NoSuchCandidate => "NoSuchCandidate",
            Error::RoundDead => "RoundDead",
            Error::AwaitingWitnesses { .. } => "AwaitingWitnesses",
            Error::State(_) => "State",
            Error::Oversize { .. } => "Oversize",
        }
    }

    /// Whether this error means "the round is not ready for that yet" rather than "that is
    /// wrong": a vote already spent, a round that has died, another party's round, or policy
    /// holding my vote for witnesses. An app retries these on the next change and does not
    /// show them; [`ChainClient::act`] retries a held proposal on exactly this test.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Error::AlreadyVoted { .. }
                | Error::RoundDead
                | Error::NotDesignated { .. }
                | Error::AwaitingWitnesses { .. }
        )
    }
}
