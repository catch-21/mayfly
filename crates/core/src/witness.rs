//! Engagements, the witness quorum and the stopwatch (§11).
//!
//! Witnessing never gates the chain. A receipt is never a validity condition for any link; a
//! link is *witnessed `m/k`* and that is reported, not enforced. Every time question — an
//! abandoned close, a recover delay, a Grant window, a premature skip — is decided by the same
//! rule: a majority of the witnesses engaged as of the committed head, each on its own
//! receipts, and only when `⌈(k+1)/2⌉` of them have receipts covering it **and agree**.
//! Otherwise the question is *asserted*, never *invalid*.

use std::collections::BTreeMap;

use crate::hash::Hash;
use crate::record::{Receipt, Signed};

/// A witness engagement as the fold knows it.
#[derive(Debug, Clone)]
pub struct Engaged {
    /// Witness pubky (z32).
    pub pubky: String,
    /// Engagement signing key (z32).
    pub kid: String,
    /// End of engagement, Unix seconds.
    pub until: u64,
    /// Polling interval; also the disagreement tolerance.
    pub poll_ms: u64,
    /// Folder under which `witness/<chain_id>/…` lives.
    pub path: String,
}

/// Receipts indexed for the stopwatch: by record hash, then by witness `kid`.
///
/// A witness is a pubky (§11.2); the `kid` that signs its receipts is a Grant client key,
/// minted afresh at each sign-in, so one witness holds several kids over a chain's life and
/// keeps every engagement it published (`engage/<kid>.jws`) so that every receipt stays
/// verifiable. Questions about *a witness's* observations therefore look across all the kids
/// the fold has established for that pubky, and the earliest observation is the one that
/// counts: a later receipt of the same record by a later key does not move the time.
#[derive(Debug, Default, Clone)]
pub struct Receipts {
    /// `record hash → (witness kid → receipt)`.
    pub by_record: BTreeMap<Hash, BTreeMap<String, Signed<Receipt>>>,
    /// `witness kid → witness pubky`, for every engagement the fold has verified.
    pub owner_of: BTreeMap<String, String>,
}

impl Receipts {
    /// Note that `kid` is `pubky`'s: an engagement under that key was verified.
    pub fn register(&mut self, kid: &str, pubky: &str) {
        self.owner_of.insert(kid.to_string(), pubky.to_string());
    }

    /// Index a verified receipt.
    pub fn insert(&mut self, receipt: Signed<Receipt>) -> Result<(), crate::Error> {
        let record = Hash::parse(&receipt.payload.record)?;
        self.by_record
            .entry(record)
            .or_default()
            .insert(receipt.payload.kid.clone(), receipt);
        Ok(())
    }

    /// `observed_at` of `record` according to the witness whose key `kid` is — the earliest
    /// receipt under any key that witness has held — if it receipted it. A kid the fold has
    /// not tied to a pubky answers for itself alone.
    pub fn observed(&self, record: &Hash, kid: &str) -> Option<u64> {
        let by_kid = self.by_record.get(record)?;
        match self.owner_of.get(kid) {
            Some(pubky) => by_kid
                .iter()
                .filter(|(k, _)| self.owner_of.get(*k) == Some(pubky))
                .map(|(_, r)| r.payload.observed_at)
                .min(),
            None => by_kid.get(kid).map(|r| r.payload.observed_at),
        }
    }

    /// How many of `engaged` receipted `record`: the `m` in *witnessed m/k*.
    pub fn witnessed(&self, record: &Hash, engaged: &[Engaged]) -> usize {
        engaged
            .iter()
            .filter(|w| self.observed(record, &w.kid).is_some())
            .count()
    }
}

/// `⌈(k+1)/2⌉`: how many of `k` engaged witnesses must have receipts and agree before a time
/// question is adjudicated rather than asserted.
pub fn quorum_size(k: usize) -> usize {
    (k + 1).div_ceil(2).max(1)
}

/// Result of putting a time question to the witness quorum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// A quorum has receipts and agrees the condition holds.
    Yes,
    /// A quorum has receipts and agrees it does not.
    No,
    /// Fewer than a quorum have receipts, or those that have disagree. Never invalid.
    Asserted {
        /// Witnesses with receipts that said yes.
        yes: usize,
        /// Witnesses with receipts that said no.
        no: usize,
        /// Engaged witnesses with no receipts covering the question.
        silent: usize,
    },
}

/// Put a per-witness boolean judgement to the quorum. `judgements[i]` is `Some(answer)` for
/// each engaged witness that had the receipts to answer, `None` for one that did not.
pub fn adjudicate(judgements: &[Option<bool>]) -> Verdict {
    let k = judgements.len();
    let need = quorum_size(k);
    let yes = judgements.iter().filter(|j| **j == Some(true)).count();
    let no = judgements.iter().filter(|j| **j == Some(false)).count();
    let silent = k - yes - no;
    if yes >= need && no == 0 {
        Verdict::Yes
    } else if no >= need && yes == 0 {
        Verdict::No
    } else {
        Verdict::Asserted { yes, no, silent }
    }
}

/// The stopwatch (§11.3). Pure functions of observed times; each charges only the party whose
/// time it was, and skew is clamped so it can shorten a clock, never lengthen it.
pub mod stopwatch {
    /// `max(0, proposal_observed − ready)` where `ready` is the latest QC confirmation of the
    /// previous link as this witness observed it.
    pub fn think(proposal_observed: u64, ready: u64) -> u64 {
        proposal_observed.saturating_sub(ready)
    }

    /// `max(0, first_vote_observed − proposal_observed)`.
    pub fn respond(first_vote_observed: u64, proposal_observed: u64) -> u64 {
        first_vote_observed.saturating_sub(proposal_observed)
    }

    /// `now − obligation_since`, per seq, ended only by a record from the party itself.
    pub fn silence(now: u64, obligation_since: u64) -> u64 {
        now.saturating_sub(obligation_since)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::hash::ChainId;
    use crate::record::{sign, Source};
    use crate::{typ, PROTOCOL_VERSION};
    use pubky_common::crypto::Keypair;

    fn receipt(kid: &Keypair, record: &Hash, observed_at: u64) -> Signed<Receipt> {
        let r = Receipt {
            v: PROTOCOL_VERSION,
            kind: "observed".into(),
            chain: ChainId::derive(b"g"),
            record: record.to_base64url(),
            typ: typ::LINK.to_string(),
            seq: Some(1),
            round: Some(0),
            by: "party".into(),
            kid: kid.public_key().z32(),
            observed_at,
            source: Source {
                pubky: "owner".into(),
                cursor: 1,
            },
            consistent: true,
        };
        Signed::decode(sign(kid, typ::WITNESS, &r).unwrap(), typ::WITNESS).unwrap()
    }

    /// One witness, two keys over its life: its observations are its own under either key,
    /// the earliest counts, and it is one witness in `m/k`, not two.
    #[test]
    fn a_witness_is_its_pubky_across_the_keys_it_has_held() {
        let record = Hash::of(b"link");
        let (old, new) = (Keypair::random(), Keypair::random());
        let (old_kid, new_kid) = (old.public_key().z32(), new.public_key().z32());
        let mut receipts = Receipts::default();
        receipts.register(&old_kid, "watchman");
        receipts.register(&new_kid, "watchman");
        receipts.insert(receipt(&old, &record, 1_000)).unwrap();
        // Asked through the key that now governs, the witness's earlier receipt answers.
        assert_eq!(receipts.observed(&record, &new_kid), Some(1_000));
        assert_eq!(receipts.observed(&record, &old_kid), Some(1_000));
        // A second receipt under the new key does not move the time.
        receipts.insert(receipt(&new, &record, 5_000)).unwrap();
        assert_eq!(receipts.observed(&record, &new_kid), Some(1_000));
        let engaged = [Engaged {
            pubky: "watchman".into(),
            kid: new_kid.clone(),
            until: u64::MAX,
            poll_ms: 5_000,
            path: "/pub/w/mayfly/".into(),
        }];
        assert_eq!(receipts.witnessed(&record, &engaged), 1);
        // A key the fold has not tied to anyone answers for itself alone.
        let stranger = Keypair::random();
        let stranger_kid = stranger.public_key().z32();
        assert_eq!(receipts.observed(&record, &stranger_kid), None);
        receipts.insert(receipt(&stranger, &record, 10)).unwrap();
        assert_eq!(receipts.observed(&record, &stranger_kid), Some(10));
        assert_eq!(
            receipts.observed(&record, &new_kid),
            Some(1_000),
            "not the stranger's"
        );
    }

    #[test]
    fn quorum_sizes() {
        assert_eq!(quorum_size(1), 1);
        assert_eq!(quorum_size(2), 2);
        assert_eq!(quorum_size(3), 2);
        assert_eq!(quorum_size(4), 3);
    }

    #[test]
    fn one_of_two_is_asserted_not_decided() {
        assert!(matches!(
            adjudicate(&[Some(true), None]),
            Verdict::Asserted {
                yes: 1,
                no: 0,
                silent: 1
            }
        ));
    }

    #[test]
    fn split_clock_is_asserted_never_invalid() {
        assert!(matches!(
            adjudicate(&[Some(true), Some(false), None]),
            Verdict::Asserted { .. }
        ));
    }

    #[test]
    fn two_of_three_agreeing_decides_through_an_outage() {
        assert_eq!(adjudicate(&[Some(true), Some(true), None]), Verdict::Yes);
        assert_eq!(adjudicate(&[None, Some(false), Some(false)]), Verdict::No);
    }

    #[test]
    fn clocks_clamp_at_zero() {
        assert_eq!(stopwatch::think(5, 10), 0);
        assert_eq!(stopwatch::respond(3, 9), 0);
    }
}
