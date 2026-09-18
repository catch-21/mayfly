//! Deterministic N-party simulator for property tests (§16.2 phase 1).
//!
//! The simulator owns keypairs and Grants for `N` parties and `k` witnesses, a fake "storage"
//! per folder, and a schedule of events — proposals, confirmations, rejects, receipts, file
//! deletions, partitions — chosen by the test's random strategy. It produces [`Inputs`] for the
//! fold from any subset of folders, so that a test can ask: does every honest verifier, given
//! any reachable subset at any moment, agree on everything that is *final*?
//!
//! Parties are honest or Byzantine per test. Honest parties follow the client discipline of
//! §6.4; Byzantine ones may equivocate, withhold, reveal late, skip early, delete their files,
//! or name anyone as an abandoned subject. Witnesses may be honest, dark, selective, or lying
//! about time.
//!
//! Skeleton: the types and the API the tests need. Bodies are filled in alongside the fold.

use std::collections::BTreeMap;

use pubky_common::crypto::Keypair;

use crate::fold::Inputs;
use crate::hash::ChainId;

/// One simulated party.
pub struct Party {
    /// Identity keypair (would live in Ring).
    pub identity: Keypair,
    /// Chain key: the Grant `cnf`.
    pub client: Keypair,
    /// Grant JWS binding `client` to `identity` with write access to `path`.
    pub grant: String,
    /// The app id.
    pub client_id: String,
    /// `/pub/<client_id>/mayfly/`.
    pub path: String,
    /// Honest or not.
    pub behaviour: Behaviour,
}

/// How a party behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Behaviour {
    /// Follows §6.4 discipline exactly.
    Honest,
    /// Votes twice in a round when it can.
    Equivocates,
    /// Withholds its confirmation and reveals it after others have moved on.
    WithholdsAndReveals,
    /// Skips every round it is allowed to, immediately.
    SkipsEarly,
    /// Rejects every valid proposal.
    Obstructs,
    /// Deletes its own files after they have been read once.
    DeletesFiles,
    /// Stops responding at a given seq.
    WalksAwayAt(u64),
}

/// One simulated witness.
pub struct Witness {
    /// Identity.
    pub identity: Keypair,
    /// Engagement signing key.
    pub client: Keypair,
    /// Its `engage.jws`.
    pub engagement: String,
    /// Behaviour.
    pub behaviour: WitnessBehaviour,
}

/// How a witness behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WitnessBehaviour {
    /// Receipts everything in causal order with a true clock.
    Honest,
    /// Never receipts.
    Dark,
    /// Receipts everything except one party's records.
    Selective(usize),
    /// Receipts with a clock offset in milliseconds.
    Skewed(i64),
    /// Issues receipts, then deletes them from its own storage.
    DeletesReceipts,
}

/// Fake storage: folder path → filename → bytes. Deletions remove entries; a party's mirror of
/// another's record is a separate entry under the mirroring party's folder.
#[derive(Debug, Default, Clone)]
pub struct Storage {
    /// Folders.
    pub folders: BTreeMap<String, BTreeMap<String, Vec<u8>>>,
}

/// A simulation in progress.
pub struct Sim {
    /// Chain id once genesis exists.
    pub chain: Option<ChainId>,
    /// Parties in genesis order.
    pub parties: Vec<Party>,
    /// Witnesses.
    pub witnesses: Vec<Witness>,
    /// All storage.
    pub storage: Storage,
    /// Simulated wall clock, Unix milliseconds.
    pub now_ms: u64,
}

impl Sim {
    /// Build `n` parties and `k` witnesses with fresh keys and Grants. `client_ids` lets a test
    /// put parties on different apps (§7); cycled if shorter than `n`.
    pub fn new(_n: usize, _k: usize, _client_ids: &[&str]) -> Self {
        unimplemented!("sim::Sim::new — filled in with the fold")
    }

    /// Everything currently in the given folders, as the fold wants it.
    pub fn inputs_from(&self, _folders: &[&str]) -> Inputs {
        unimplemented!("sim::Sim::inputs_from")
    }

    /// Everything in every folder.
    pub fn inputs_all(&self) -> Inputs {
        let folders: Vec<&str> = self.storage.folders.keys().map(String::as_str).collect();
        self.inputs_from(&folders)
    }
}
