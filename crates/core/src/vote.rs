//! Rounds, votes, death, rotation and quorum certificates (§6.4).
//!
//! These are the pure rules the fold applies. They are small on purpose and are the first
//! things the property tests pin down:
//!
//! - one vote per party per round, never taken back;
//! - a round with proposals is dead when no link can still reach `q`;
//! - a round with no proposal is dead only by `N − q + 1` rejects (pass or skip);
//! - rounds `≥ 1` have one designated proposer, by rotation over genesis parties;
//! - a QC is the author's proposal plus confirmations in the same round reaching `q`
//!   (`N` for a `witnesses` link);
//! - commitment consults QCs only — never death evidence, never receipts.

use std::collections::{BTreeMap, BTreeSet};

use crate::hash::{ChainId, Hash};
use crate::record::Kind;
use crate::rules::PartyIndex;

/// What a party's single vote in a round was for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vote {
    /// Proposed, or confirmed, this link.
    For(Hash),
    /// Refused a proposal, passed, or skipped.
    Nothing,
}

/// The votes cast in one `(seq, round)`, keyed by party. A second vote by the same party is
/// equivocation; the fold records it and keeps counting (§6.4: a QC is a QC).
#[derive(Debug, Default, Clone)]
pub struct RoundVotes {
    /// Every vote each party cast in this round, in the order seen. Length > 1 = equivocation.
    pub by_party: BTreeMap<PartyIndex, Vec<Vote>>,
    /// Links proposed in this round, with their authors.
    pub proposals: BTreeMap<Hash, PartyIndex>,
}

impl RoundVotes {
    /// Record a vote.
    pub fn cast(&mut self, party: PartyIndex, vote: Vote) {
        self.by_party.entry(party).or_default().push(vote);
    }

    /// Record a proposal (which is also the author's vote for it).
    pub fn propose(&mut self, party: PartyIndex, link: Hash) {
        self.proposals.insert(link, party);
        self.cast(party, Vote::For(link));
    }

    /// Parties that voted at all.
    pub fn voters(&self) -> BTreeSet<PartyIndex> {
        self.by_party.keys().copied().collect()
    }

    /// Distinct parties who voted for `link` (an equivocator counts once per link).
    pub fn votes_for(&self, link: &Hash) -> usize {
        self.by_party
            .values()
            .filter(|vs| vs.contains(&Vote::For(*link)))
            .count()
    }

    /// Distinct parties who voted for nothing (a reject, pass or skip).
    pub fn rejects(&self) -> usize {
        self.by_party
            .values()
            .filter(|vs| vs.contains(&Vote::Nothing))
            .count()
    }

    /// Parties with more than one vote in this round.
    pub fn equivocators(&self) -> Vec<PartyIndex> {
        self.by_party
            .iter()
            .filter(|(_, vs)| vs.len() > 1)
            .map(|(p, _)| *p)
            .collect()
    }

    /// Is this round dead (§6.4)?
    ///
    /// With at least one proposal: for every proposed `L`,
    /// `votes(L) + (N − voters) < q`. With none: `rejects >= N − q + 1`. Never quantified over
    /// an empty set of links.
    pub fn is_dead(&self, n: usize, q: usize) -> bool {
        let voters = self.voters().len();
        if self.proposals.is_empty() {
            return self.rejects() >= n + 1 - q;
        }
        self.proposals
            .keys()
            .all(|l| self.votes_for(l) + (n - voters) < q)
    }

    /// Links in this round that hold a quorum certificate: `votes_for(L) >= q`.
    pub fn quorum_certified(&self, q: usize) -> Vec<Hash> {
        self.proposals
            .keys()
            .filter(|l| self.votes_for(l) >= q)
            .copied()
            .collect()
    }

    /// The lowest-hash link that received any vote — what the next round's designated proposer
    /// re-proposes (§6.4). `None` if the round died by pass or skip alone.
    pub fn lowest_voted(&self) -> Option<Hash> {
        self.proposals
            .keys()
            .filter(|l| self.votes_for(l) > 0)
            .min()
            .copied()
    }
}

/// The effective quorum for a link kind (§9.2 step 3b): `N` for `witnesses`, else genesis `q`.
pub fn effective_quorum(kind: Kind, n: usize, q: usize) -> usize {
    match kind {
        Kind::Witnesses => n,
        _ => q,
    }
}

/// The designated proposer of round `r ≥ 1` at `seq` (§6.4):
/// `parties[(u64_be(BLAKE3(ascii(chain_id) ‖ u64_be(seq))[0..8]) + r) mod N]`.
pub fn designated(chain: &ChainId, seq: u64, round: u32, n: usize) -> PartyIndex {
    debug_assert!(round >= 1, "round 0 has no designated proposer");
    let mut hasher = pubky_common::crypto::Hasher::new();
    hasher.update(chain.ascii());
    hasher.update(&seq.to_be_bytes());
    let digest = hasher.finalize();
    let mut first8 = [0u8; 8];
    first8.copy_from_slice(&digest.as_bytes()[..8]);
    let base = u64::from_be_bytes(first8);
    ((base.wrapping_add(round as u64)) % n as u64) as PartyIndex
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(b: u8) -> Hash {
        Hash::from_bytes([b; 32])
    }

    #[test]
    fn empty_round_not_dead_until_a_reject() {
        let r = RoundVotes::default();
        assert!(!r.is_dead(3, 3), "empty ∀ must not be vacuously dead");
        let mut r = r;
        r.cast(1, Vote::Nothing);
        assert!(r.is_dead(3, 3));
    }

    #[test]
    fn unanimity_two_different_votes_kill_a_round() {
        let mut r = RoundVotes::default();
        r.propose(0, h(1));
        assert!(!r.is_dead(3, 3));
        r.propose(1, h(2));
        assert!(r.is_dead(3, 3));
        assert_eq!(r.lowest_voted(), Some(h(1)));
    }

    #[test]
    fn quorum_mode_needs_more_rejects_for_empty_death() {
        let mut r = RoundVotes::default();
        r.cast(0, Vote::Nothing);
        assert!(!r.is_dead(5, 3), "N − q + 1 = 3 rejects needed");
        r.cast(1, Vote::Nothing);
        r.cast(2, Vote::Nothing);
        assert!(r.is_dead(5, 3));
    }

    #[test]
    fn qc_forms_at_q() {
        let mut r = RoundVotes::default();
        r.propose(0, h(1));
        r.cast(1, Vote::For(h(1)));
        assert!(r.quorum_certified(3).is_empty());
        r.cast(2, Vote::For(h(1)));
        assert_eq!(r.quorum_certified(3), vec![h(1)]);
    }

    #[test]
    fn equivocation_counts_but_is_recorded() {
        let mut r = RoundVotes::default();
        r.propose(0, h(1));
        r.cast(1, Vote::For(h(1)));
        r.cast(1, Vote::Nothing);
        assert_eq!(r.equivocators(), vec![1]);
        assert_eq!(r.votes_for(&h(1)), 2);
    }

    #[test]
    fn rotation_is_deterministic_and_covers_all_parties() {
        let chain = ChainId::derive(b"g");
        let a = designated(&chain, 7, 1, 3);
        assert_eq!(a, designated(&chain, 7, 1, 3));
        let mut seen = BTreeSet::new();
        for r in 1..=3 {
            seen.insert(designated(&chain, 7, r, 3));
        }
        assert_eq!(seen.len(), 3);
    }

    #[test]
    fn witnesses_kind_needs_everyone() {
        assert_eq!(effective_quorum(Kind::Witnesses, 5, 3), 5);
        assert_eq!(effective_quorum(Kind::Rules, 5, 3), 3);
    }
}
