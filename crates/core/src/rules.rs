//! The rules engine interface (§10).
//!
//! Rules are a deterministic pure function over the chain. No clocks, no randomness, no
//! floating point, no unordered iteration in canonical output. Because the chain is linear and
//! committed one link at a time, rules never resolve concurrent edits. Protocol kinds — `rekey`,
//! `recover`, `reveal`, `close`, `witnesses` — never reach `apply`.

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::genesis::Genesis;
use crate::record::{Confirmation, Link};

/// Index into `genesis.parties`.
pub type PartyIndex = usize;

/// A nonce revealed by a `reveal` link (§6.6).
pub type Nonce = Vec<u8>;

/// Whether the rules consider the chain finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// More links may follow.
    Ongoing,
    /// Terminal; a `close {finished}` is now valid.
    Finished(Outcome),
}

/// What a chain ended with. Rules-defined text plus an optional winner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Rules-specific description, e.g. `"1-0"`, `"archived"`, `"lapsed"`.
    pub summary: String,
    /// Parties the outcome favours, if the rules have that notion.
    pub winners: Vec<PartyIndex>,
}

/// Errors from a rules implementation.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct RulesError(pub String);

/// Body of a `close` as the rules see it.
pub use crate::record::CloseBody;

/// A rules implementation for one pinned rules id.
pub trait Rules {
    /// Deterministic state.
    type State: Clone + Serialize + DeserializeOwned;
    /// Rules-specific link body.
    type Body: Serialize + DeserializeOwned;

    /// Stable id pinned in genesis, e.g. `"list/1"`.
    fn id(&self) -> &'static str;

    /// BLAKE3 of the reference module for `id()`; genesis must pin it (§6.6).
    fn reference_hash(&self) -> &'static str;

    /// Validate genesis (roles, options) and build the initial state once genesis is committed
    /// and, if the rules asked for randomness, every party's `reveal` is in (§6.6).
    fn init(
        &self,
        genesis: &Genesis,
        confirmations: &[Confirmation],
        nonces: &[Nonce],
    ) -> Result<Self::State, RulesError>;

    /// Whether these rules need commit-reveal nonces before `init` (chess seat randomisation
    /// when roles are not fixed).
    fn wants_reveals(&self, genesis: &Genesis) -> bool;

    /// Parties *expected* to propose next (chess: the side to move; a list: nobody). Not
    /// eligibility — it feeds only the stopwatch (`think`, `silence`, §11.3).
    fn obliged(&self, state: &Self::State) -> Vec<PartyIndex>;

    /// May `party` propose a link of `kind` now? The eligibility gate for rules content in every
    /// round (chess: `move` only for the side to move; `resign` for either side).
    fn may_append(&self, state: &Self::State, party: PartyIndex, kind: &str) -> bool;

    /// Deterministic, pure transition.
    fn apply(&self, state: &Self::State, link: &Link) -> Result<Self::State, RulesError>;

    /// Ongoing | Finished(outcome).
    fn status(&self, state: &Self::State) -> Status;

    /// Outcome recorded by a `close` (§6.8); also validates it (a `finished` close is only
    /// valid when `status` is `Finished`). For `abandoned`, decides what the subjects' absence
    /// means: chess returns a loss by default for the subject.
    fn close(&self, state: &Self::State, close: &CloseBody) -> Result<Outcome, RulesError>;

    /// Deterministic bytes for hashing `state`.
    fn canonical_state(&self, state: &Self::State) -> Vec<u8>;
}

/// `BLAKE3(canonical_state(state))` as written in `Link::state` and `Confirmation::state`.
pub fn state_hash<R: Rules>(rules: &R, state: &R::State) -> crate::Hash {
    crate::Hash::of(&rules.canonical_state(state))
}
