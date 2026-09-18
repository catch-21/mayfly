//! Property tests: the invariants six reviews of `docs/MAYFLY.md` converged on.
//!
//! Each test names the section it pins. Tests marked `#[ignore]` are waiting on the fold and the
//! simulator; they are the specification of what those must satisfy, written before the code.
//! Run the live ones with `cargo test -p pubky-mayfly --test invariants`, and everything with
//! `cargo test -p pubky-mayfly --test invariants -- --ignored`.
//!
//! Pure-rule invariants (rounds, death, rotation, quorum) run now against `vote` and `witness`.

use std::collections::BTreeSet;

use proptest::prelude::*;

use pubky_mayfly::hash::{ChainId, Hash};
use pubky_mayfly::vote::{designated, RoundVotes, Vote};
use pubky_mayfly::witness::{adjudicate, quorum_size, Verdict};

fn hash_strategy() -> impl Strategy<Value = Hash> {
    any::<[u8; 32]>().prop_map(Hash::from_bytes)
}

/// A random round: `n` parties, each casting 0..=2 votes drawn from a small pool of links or
/// "nothing". Two votes from one party is deliberate: equivocation must be countable.
fn round_strategy(n: usize) -> impl Strategy<Value = RoundVotes> {
    let links = proptest::collection::vec(hash_strategy(), 1..=3);
    (links, proptest::collection::vec(0u8..=2, n)).prop_flat_map(move |(links, counts)| {
        let n_links = links.len();
        let per_party = counts
            .iter()
            .map(|c| proptest::collection::vec(0usize..=n_links, *c as usize))
            .collect::<Vec<_>>();
        per_party.prop_map(move |choices| {
            let mut r = RoundVotes::default();
            for (party, votes) in choices.iter().enumerate() {
                for &v in votes {
                    if v == n_links {
                        r.cast(party, Vote::Nothing);
                    } else {
                        let l = links[v];
                        if r.proposals.contains_key(&l) {
                            r.cast(party, Vote::For(l));
                        } else {
                            r.propose(party, l);
                        }
                    }
                }
            }
            r
        })
    })
}

proptest! {
    /// §6.4: two QCs in one round require at least `2q − N` double voters.
    #[test]
    fn two_qcs_in_one_round_need_double_votes(n in 2usize..=6, r in round_strategy(6)) {
        prop_assume!(r.by_party.keys().all(|p| *p < n));
        for q in (n / 2 + 1)..=n {
            let qcs = r.quorum_certified(q);
            if qcs.len() >= 2 {
                let equivocators = r.equivocators().len();
                prop_assert!(
                    equivocators >= 2 * q - n,
                    "two QCs with {equivocators} equivocators, q={q}, n={n}"
                );
            }
        }
    }

    /// §6.4: a round with no proposal is never dead by vacuity; it dies only by `N − q + 1`
    /// rejects.
    #[test]
    fn empty_round_death_needs_rejects(n in 2usize..=6, rejects in 0usize..=6) {
        let rejects = rejects.min(n);
        let mut r = RoundVotes::default();
        for p in 0..rejects {
            r.cast(p, Vote::Nothing);
        }
        for q in (n / 2 + 1)..=n {
            prop_assert_eq!(r.is_dead(n, q), rejects >= n + 1 - q);
        }
    }

    /// §6.4: a round holding a QC is never dead. Death and commitment are exclusive.
    #[test]
    fn a_qc_round_is_alive(n in 2usize..=6, r in round_strategy(6)) {
        prop_assume!(r.by_party.keys().all(|p| *p < n));
        for q in (n / 2 + 1)..=n {
            if !r.quorum_certified(q).is_empty() && r.equivocators().is_empty() {
                prop_assert!(!r.is_dead(n, q));
            }
        }
    }

    /// §6.4: rotation over genesis parties is deterministic and, over `N` consecutive rounds,
    /// designates every party exactly once.
    #[test]
    fn rotation_covers_every_party(genesis in any::<[u8; 16]>(), seq in any::<u64>(), n in 2usize..=8) {
        let chain = ChainId::derive(&genesis);
        let seen: BTreeSet<_> = (1..=n as u32).map(|r| designated(&chain, seq, r, n)).collect();
        prop_assert_eq!(seen.len(), n);
        prop_assert_eq!(designated(&chain, seq, 1, n), designated(&chain, seq, 1, n));
    }

    /// §11.2: a single receipt never decides a time question; fewer than `⌈(k+1)/2⌉` agreeing
    /// witnesses is *asserted*, and any disagreement is *asserted*, never `No`.
    #[test]
    fn one_receipt_decides_nothing(judgements in proptest::collection::vec(proptest::option::of(any::<bool>()), 1..=5)) {
        let k = judgements.len();
        let yes = judgements.iter().filter(|j| **j == Some(true)).count();
        let no = judgements.iter().filter(|j| **j == Some(false)).count();
        match adjudicate(&judgements) {
            Verdict::Yes => { prop_assert!(yes >= quorum_size(k) && no == 0); }
            Verdict::No => { prop_assert!(no >= quorum_size(k) && yes == 0); }
            Verdict::Asserted { .. } => { prop_assert!(yes < quorum_size(k) || no > 0 || no < quorum_size(k)); }
        }
        if k >= 2 {
            prop_assert_ne!(adjudicate(&[Some(true)].into_iter().chain(std::iter::repeat_n(None, k - 1)).collect::<Vec<_>>()), Verdict::Yes);
        }
    }
}

// ─── Waiting on the fold and simulator ───────────────────────────────────────────────────────
//
// Each of these is one sentence from the specification. They are `#[ignore]`d until
// `fold::verify` and `sim::Sim` exist; the message says which section they pin.

#[test]
#[ignore = "§6.4 / §9.2 step 3d: commitment consults votes only — needs fold + sim"]
fn commitment_is_independent_of_receipts_and_death_evidence() {
    // For a random honest run, folding with any subset of receipts, and with any rejects
    // deleted, yields the same `committed[]` (as hashes) as folding with everything.
}

#[test]
#[ignore = "§6.4: honest parties converge within two rounds at every seq — needs sim"]
fn honest_parties_converge_within_two_rounds() {}

#[test]
#[ignore = "§6.4: a skip pushes a seq at most N − 1 rounds — needs sim with SkipsEarly"]
fn skips_are_bounded() {}

#[test]
#[ignore = "§6.4 / §9.2: a withheld vote revealed late never changes a link with a committed successor"]
fn history_is_immutable() {}

#[test]
#[ignore = "§11.2: nothing waits for a third party — a Dark witness never changes committed[] or status"]
fn dark_witness_changes_nothing_but_witnessed_counts() {}

#[test]
#[ignore = "§6.8: only an adjudicated close is final; asserted/contested pauses — two verifiers with different receipt sets never reach contradictory finals"]
fn close_verdicts_never_contradict_when_final() {}

#[test]
#[ignore = "§6.8: presence is decided from files — a subject record at the seq always contests an asserted close"]
fn presence_from_files() {}

#[test]
#[ignore = "§6.7: old-key veto voids a provisional recover and is an anomaly after it is history"]
fn recover_veto_window() {}

#[test]
#[ignore = "§9.2 step 2: per-signer bound — a mirrored QC never marks the folder owner hostile"]
fn mirrored_qc_is_not_hostile() {}

#[test]
#[ignore = "§6: any single byte mutation anywhere fails verification of that record"]
fn any_byte_mutation_fails() {}

#[test]
#[ignore = "§7: parties on different client_ids; a recover moves a party's folder and the fold follows"]
fn folders_follow_declared_paths() {}
