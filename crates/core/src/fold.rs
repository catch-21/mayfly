//! The verifier (§9.2): anyone, from files alone.
//!
//! Inputs are bytes the caller found — links, confirmations, rejects, engagements, receipts,
//! revocations — from any source and in any order. Output is a [`Verdict`]: the committed chain,
//! its status, and every anomaly attributed to a key. The fold never trusts where a file came
//! from; it trusts signatures, hashes and quorums.
//!
//! The two principles every step obeys (§6.4, §11.2):
//!
//! 1. **Commitment consults votes only.** Step 3d reads proposals, confirmations and rejects.
//!    Death evidence, receipts and skips' fairness are reported, never inputs to which link is
//!    committed.
//! 2. **Nothing waits for a third party.** No receipt is a validity condition. Every time
//!    verdict is provisional until the records it touches are final, and is reached by the
//!    witness quorum, never one receipt.
//!
//! This file is the skeleton: types are final, the algorithm is written as the specification's
//! numbered steps with the pure helpers it calls, and the body returns
//! [`Error::Unimplemented`] until the property tests in `tests/invariants.rs` drive it in.

use std::collections::BTreeMap;

use crate::close::CloseState;
use crate::error::Error;
use crate::hash::{ChainId, Hash};
use crate::keys::Seat;
use crate::record::{Confirmation, Engagement, Link, Receipt, Reject, Signed};
use crate::rules::{Outcome, PartyIndex, Rules};
use crate::witness::Engaged;

/// Everything the caller could find. Bytes only; the fold decodes and verifies.
#[derive(Debug, Default, Clone)]
pub struct Inputs {
    /// Contents of every `links/*.jws` from every declared path, plus any watchdog mirror.
    pub links: Vec<Vec<u8>>,
    /// Contents of every `confirms/*.jws`.
    pub confirms: Vec<Vec<u8>>,
    /// Contents of every `rejects/*.jws`.
    pub rejects: Vec<Vec<u8>>,
    /// Every `engage.jws` found: current, historic, and party-mirrored.
    pub engagements: Vec<Vec<u8>>,
    /// Every receipt found: on witnesses, mirrored by parties, embedded in links.
    pub receipts: Vec<Vec<u8>>,
    /// Every `keys/<kid>.revoked.jws` found, capped per established kid (§9.2 step 2).
    pub revocations: Vec<Vec<u8>>,
    /// Which folder each byte-blob was read from, for hostile-source accounting. Optional.
    pub sources: BTreeMap<Hash, String>,
}

/// Where the chain stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Head is committed and the next seq is open.
    Ongoing,
    /// No QC at `seq` and no close; reports the current round and who has not voted.
    Stalled {
        /// The open seq.
        seq: u64,
        /// Highest round with any vote.
        round: u32,
        /// Whether that round is dead in the files.
        dead: bool,
        /// Parties who have not voted in it.
        awaiting: Vec<PartyIndex>,
    },
    /// An asserted or contested abandoned close sits at `seq`; provisional.
    Paused {
        /// The seq of the close.
        seq: u64,
        /// Its state.
        close: CloseState,
    },
    /// Ended by `close {finished}` or `close {agreed}`.
    Closed(Outcome),
    /// Ended by an adjudicated `close {abandoned}`.
    Abandoned {
        /// Who was closed out.
        subjects: Vec<PartyIndex>,
        /// The rules' outcome.
        outcome: Outcome,
    },
}

/// A committed link with what the fold knows about it.
#[derive(Debug, Clone)]
pub struct Committed {
    /// The link.
    pub link: Signed<Link>,
    /// Confirmations forming its QC.
    pub qc: Vec<Signed<Confirmation>>,
    /// True once a successor has embedded this QC (§6.4).
    pub is_final: bool,
    /// *Witnessed m/k*: how many of the engaged witnesses receipted its QC-completing
    /// confirmation, over how many were engaged.
    pub witnessed: (usize, usize),
}

/// Misbehaviour or inconsistency, attributed. Never stops the fold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anomaly {
    /// Key (party `kid` or witness `kid`) the anomaly is attributed to; `None` if a source.
    pub against: Option<String>,
    /// Seq it concerns, if any.
    pub seq: Option<u64>,
    /// What happened.
    pub kind: AnomalyKind,
    /// Hashes of the records that prove it.
    pub evidence: Vec<Hash>,
}

/// The catalogue of §9.2 step 5 and §11.6.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnomalyKind {
    /// Two votes by one key in one round.
    Equivocation,
    /// Mirrored bytes differ from the original.
    TamperedMirror,
    /// A valid proposal nobody confirmed.
    UnconfirmedProposal,
    /// A round `> 0` whose predecessor is not dead in the files.
    UnjustifiedRound,
    /// A skip before the designated party had been silent for `think_ms` (receipts only).
    PrematureSkip,
    /// A reject of a link the rules find valid.
    Obstruction,
    /// A confirmation whose `state` differs from the link's.
    RulesDivergence,
    /// An abandoned close that named a present party, or exceeded the subject bound.
    VoidClose,
    /// A subject record observed after an adjudicated close.
    LateSubject,
    /// A recover confirmed inside `recovery_delay_ms` (quorum-adjudicated).
    PrematureRecover,
    /// An old-key reject after the recover became history.
    LateVeto,
    /// A record a witness quorum places after its Grant's `exp` or a revocation.
    GrantWindow,
    /// A folder past the per-signer bound; read no further.
    HostileSource,
    /// A witness that never receipted, receipted selectively, deleted a receipt, or contradicts
    /// other witnesses (§11.6 table).
    Witness(String),
    /// Two engagements for one witness pubky with contradictory `until`.
    WitnessEquivocation,
}

/// The fold's output.
#[derive(Debug, Clone)]
pub struct Verdict {
    /// Chain id derived from genesis bytes.
    pub chain: ChainId,
    /// Committed links, seq 0 upward.
    pub committed: Vec<Committed>,
    /// Where the chain stands.
    pub status: Status,
    /// Every seat as established at the head.
    pub seats: Vec<Seat>,
    /// Every witness engaged at the head.
    pub engaged: Vec<Engaged>,
    /// Every anomaly, attributed.
    pub anomalies: Vec<Anomaly>,
}

/// Configuration the verifier itself owns (not genesis parameters).
#[derive(Debug, Clone)]
pub struct Config {
    /// Largest record this verifier will read regardless of genesis (§6.6).
    pub max_body_cap: u64,
    /// Per signer, per `(seq, round)`: records beyond this mark the source hostile (§9.2).
    pub records_per_signer_round: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_body_cap: crate::genesis::DEFAULT_MAX_BODY_BYTES,
            records_per_signer_round: 3,
        }
    }
}

/// Verify a chain from files alone.
///
/// `rules` must implement the id genesis pins, or the genesis is invalid (§6.6). The steps
/// below are the specification's, kept as the function's structure so that a reader can hold
/// the two side by side.
pub fn verify<R: Rules>(_rules: &R, _inputs: &Inputs, _config: &Config) -> Result<Verdict, Error> {
    // 1. Genesis: decode the lowest-seq link with prev == "", verify the initiator's Grant
    //    (§5.2), derive chain_id, check safety parameters (§6.6).
    // 2. Decode every record; dedupe by hash; apply the per-signer bound; establish genesis
    //    seats from genesis confirmations; establish GENESIS witnesses' engagements only;
    //    index receipt bytes (usable once their engagement is established); list revocations
    //    as discovery only.
    // 3. Fold seq by seq:
    //      a. candidates (eligibility per §6.4; protocol kinds never gated by the rules);
    //      b. QCs at effective_quorum(kind);
    //      c. votes per (party, round); vetoes set aside; equivocation recorded, votes count;
    //      d. COMMITTED[seq]: a successor's embedded QC decides (history), else the QC in the
    //         highest round (provisional head); QCs only — never death evidence or receipts;
    //      e. embedded QC of prev is valid; present receipts verified; no completeness;
    //      f. protocol kinds: rekey / recover (veto while provisional; delay by quorum) /
    //         reveal / witnesses (seat from next seq) / close finished|agreed; rules kinds:
    //         may_append, apply, state hash;
    //      g. unjustified rounds and premature skips: anomalies only;
    //      h. abandoned close: bound, presence from files, adjudication by quorum; only
    //         adjudicated-valid ends the chain, asserted/contested pauses it.
    // 4. Grant windows by the witness quorum; provisional until final; embedded QCs are history.
    // 5. Assemble the Verdict.
    Err(Error::Unimplemented(
        "fold::verify — driven in by tests/invariants.rs",
    ))
}

/// Decode and index one link's bytes. Exposed so the client can run the single-link check
/// (§9.3) without a full fold.
pub fn decode_link(bytes: Vec<u8>) -> Result<Signed<Link>, Error> {
    Signed::decode(bytes, crate::typ::LINK)
}

/// Decode a confirmation.
pub fn decode_confirmation(bytes: Vec<u8>) -> Result<Signed<Confirmation>, Error> {
    Signed::decode(bytes, crate::typ::CONFIRM)
}

/// Decode a reject.
pub fn decode_reject(bytes: Vec<u8>) -> Result<Signed<Reject>, Error> {
    Signed::decode(bytes, crate::typ::REJECT)
}

/// Decode a receipt.
pub fn decode_receipt(bytes: Vec<u8>) -> Result<Signed<Receipt>, Error> {
    Signed::decode(bytes, crate::typ::WITNESS)
}

/// Decode an engagement.
pub fn decode_engagement(bytes: Vec<u8>) -> Result<Signed<Engagement>, Error> {
    Signed::decode(bytes, crate::typ::WITNESS)
}
