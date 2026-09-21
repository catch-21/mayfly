//! Mayfly client (§7, §8): the I/O half.
//!
//! The core crate decides what bytes mean; this crate moves bytes. It owns the storage layout
//! under `/pub/<client_id>/mayfly/`, the propose / confirm / reject / mirror flows of §8, the
//! "sync before voting" merge of every reachable source, change watching with a polling
//! fallback, and — later — watchdog engagement.
//!
//! Everything is written against two small traits so the same flows run over an in-memory
//! store in tests and over the Pubky SDK in an app:
//!
//! - [`Store`]: read anyone's `/pub/` folder, write my own ([`MemoryStore`], [`PubkyStore`]);
//! - [`Signer`]: the party's identity, chain key and app, and a JWS signature under that key
//!   ([`LocalSigner`], [`SessionSigner`] over a Pubky grant session — §16.3).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod chain;
pub mod layout;
pub mod pubky_store;
pub mod signer;
pub mod store;

pub use chain::{Action, ChainClient, GenesisSpec, Policy, SyncReport};
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
}
