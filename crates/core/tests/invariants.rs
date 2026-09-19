//! Property tests: the invariants six reviews of `docs/MAYFLY.md` converged on.
//!
//! Each test names the section it pins. Tests marked `#[ignore]` are waiting on parts of the
//! fold and the simulator that do not exist yet; they are the specification of what those must
//! satisfy, written before the code. Run the live ones with
//! `cargo test -p pubky-mayfly --test invariants`, and everything with
//! `cargo test -p pubky-mayfly --test invariants -- --ignored`.
//!
//! Pure-rule invariants (rounds, death, rotation, quorum) run against `vote` and `witness`.
//! Fold invariants run honest simulations and compare `fold::verify` with the simulator's own
//! model, then with itself under different inputs.

use std::collections::BTreeSet;

use proptest::prelude::*;

use pubky_mayfly::fold::{verify, AnomalyKind, Config, Inputs, Status};
use pubky_mayfly::hash::{ChainId, Hash};
use pubky_mayfly::sim::{Behaviour, Choice, RandomTally, SeqPlan, Sim, Tally, WitnessBehaviour};
use pubky_mayfly::vote::{designated, RoundVotes, Vote};
use pubky_mayfly::witness::{adjudicate, quorum_size, Verdict};
use pubky_mayfly::Error;

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
    fn two_qcs_in_one_round_need_double_votes((n, r) in (2usize..=6).prop_flat_map(|n| (Just(n), round_strategy(n)))) {
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
    fn a_qc_round_is_alive((n, r) in (2usize..=6).prop_flat_map(|n| (Just(n), round_strategy(n)))) {
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

// ─── Honest simulations ───────────────────────────────────────────────────────────────────────

/// A random honest run: `n` parties on two apps, `k` honest witnesses, quorum `q`, and a plan
/// per seq. Masks pick the receipt and reject subsets a second verifier gets to see.
#[derive(Debug, Clone)]
struct Run {
    n: usize,
    k: usize,
    q: usize,
    plans: Vec<SeqPlan>,
    receipt_mask: u64,
    reject_mask: u64,
}

fn plan_strategy(n: usize, max_passes: u32) -> impl Strategy<Value = SeqPlan> {
    (
        proptest::collection::btree_set(0..n, 1..=n.min(3)),
        proptest::collection::vec(0u64..100, 3),
        proptest::collection::vec((any::<bool>(), 0usize..3), n),
        0u32..=max_passes,
    )
        .prop_map(|(proposers, values, choices, passes)| SeqPlan {
            proposers: proposers.into_iter().collect(),
            values,
            choices: choices
                .into_iter()
                .map(|(reject, i)| {
                    if reject {
                        Choice::Reject(i)
                    } else {
                        Choice::Confirm(i)
                    }
                })
                .collect(),
            passes,
            silent_wait_ms: 0,
        })
}

fn run_strategy(max_k: usize, max_passes: u32) -> impl Strategy<Value = Run> {
    (2usize..=4, 0usize..=max_k).prop_flat_map(move |(n, k)| {
        (
            Just(n),
            Just(k),
            (n / 2 + 1)..=n,
            proptest::collection::vec(plan_strategy(n, max_passes), 1..=5),
            any::<u64>(),
            any::<u64>(),
        )
            .prop_map(|(n, k, q, plans, receipt_mask, reject_mask)| Run {
                n,
                k,
                q,
                plans,
                receipt_mask,
                reject_mask,
            })
    })
}

/// Build and play a run. Returns the simulator and the rounds each seq took.
fn play(run: &Run) -> Result<(Sim<Tally>, Vec<u32>), TestCaseError> {
    let mut sim = Sim::new(
        Tally,
        run.n,
        run.k,
        &["chess.example", "notes.example"],
        run.q,
    );
    sim.bootstrap()
        .map_err(|e| TestCaseError::fail(e.to_string()))?;
    let mut rounds = Vec::new();
    for plan in &run.plans {
        rounds.push(
            sim.play(plan)
                .map_err(|e| TestCaseError::fail(e.to_string()))?,
        );
    }
    Ok((sim, rounds))
}

fn keep(mask: u64, i: usize) -> bool {
    mask & (1 << (i % 64)) != 0
}

fn config() -> Config {
    Config::default()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, .. ProptestConfig::default() })]

    /// §6.4 / §9.2 step 3d: commitment consults votes only. For a random honest run, folding
    /// with any subset of receipts, and with any rejects deleted, yields the same `committed[]`
    /// (as hashes) and the same status as folding with everything — and both agree with the
    /// simulator's own model.
    #[test]
    fn commitment_is_independent_of_receipts_and_death_evidence(run in run_strategy(2, 2)) {
        let (sim, _) = play(&run)?;
        let all = sim.inputs_all();
        let full = verify(&Tally, &all, &config()).map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(full.committed_hashes(), sim.committed_hashes());
        prop_assert_eq!(&full.status, &Status::Ongoing);
        prop_assert!(full.anomalies.is_empty(), "honest run flagged: {:?}", full.anomalies);

        let mut partial = all.clone();
        partial.receipts = partial
            .receipts
            .into_iter()
            .enumerate()
            .filter(|(i, _)| keep(run.receipt_mask, *i))
            .map(|(_, b)| b)
            .collect();
        partial.rejects = partial
            .rejects
            .into_iter()
            .enumerate()
            .filter(|(i, _)| keep(run.reject_mask, *i))
            .map(|(_, b)| b)
            .collect();
        let v = verify(&Tally, &partial, &config()).map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(v.committed_hashes(), full.committed_hashes());
        prop_assert_eq!(&v.status, &full.status);
        // Deleted rejects may make a round look unjustified, or a pass round's empty rejects
        // look like skips (premature, or one too many); those are reports, never inputs.
        prop_assert!(v
            .anomalies
            .iter()
            .all(|a| matches!(a.kind, AnomalyKind::UnjustifiedRound | AnomalyKind::PrematureSkip | AnomalyKind::InvalidSkip)), "{:?}", v.anomalies);
    }

    /// §6.4: honest parties converge within two rounds at every seq, and an honest run
    /// produces no anomaly.
    #[test]
    fn honest_parties_converge_within_two_rounds(run in run_strategy(1, 0)) {
        let (sim, rounds) = play(&run)?;
        prop_assert!(rounds.iter().all(|r| *r <= 2), "rounds {:?}", rounds);
        let v = verify(&Tally, &sim.inputs_all(), &config()).map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(v.committed_hashes(), sim.committed_hashes());
        prop_assert!(v.committed.iter().all(|c| c.link.payload.round <= 1));
        prop_assert!(v.anomalies.is_empty(), "{:?}", v.anomalies);
        // Every link but the head is final; the head is provisional.
        let finals: Vec<bool> = v.committed.iter().map(|c| c.is_final).collect();
        prop_assert_eq!(finals.last(), Some(&false));
        prop_assert!(finals[..finals.len() - 1].iter().all(|f| *f));
    }

    /// §6: any single byte mutation anywhere fails verification of that record, and — because
    /// every committed link and QC is mirrored (§7) — never changes what is committed.
    #[test]
    fn any_byte_mutation_fails(run in run_strategy(1, 1), which in any::<u32>(), at in any::<u32>(), xor in 1u8..=255) {
        let (sim, _) = play(&run)?;
        let all = sim.inputs_all();
        let full = verify(&Tally, &all, &config()).map_err(|e| TestCaseError::fail(e.to_string()))?;

        let mut mutated = all.clone();
        let buckets: Vec<&mut Vec<Vec<u8>>> = vec![
            &mut mutated.links,
            &mut mutated.confirms,
            &mut mutated.rejects,
            &mut mutated.engagements,
            &mut mutated.receipts,
        ];
        let total: usize = buckets.iter().map(|b| b.len()).sum();
        let mut idx = which as usize % total;
        let mut mutated_hash = None;
        for bucket in buckets {
            if idx < bucket.len() {
                let blob = &mut bucket[idx];
                let pos = at as usize % blob.len();
                blob[pos] ^= xor;
                mutated_hash = Some(Hash::of(blob));
                break;
            }
            idx -= bucket.len();
        }
        let mutated_hash = mutated_hash.expect("one blob mutated");

        let v = verify(&Tally, &mutated, &config()).map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(v.committed_hashes(), full.committed_hashes());
        prop_assert_eq!(&v.status, &full.status);
        for c in &v.committed {
            prop_assert_ne!(c.link.hash, mutated_hash);
            prop_assert!(c.qc.iter().all(|q| q.hash != mutated_hash));
        }
    }
}

/// §11.2: nothing waits for a third party. A Dark witness never changes `committed[]` or the
/// status; it only lowers `witnessed m/k`. Stripping every receipt does the same.
#[test]
fn dark_witness_changes_nothing_but_witnessed_counts() {
    let mut sim = Sim::new(Tally, 3, 3, &["chess.example"], 3);
    sim.witnesses[1].behaviour = WitnessBehaviour::Dark;
    sim.bootstrap().unwrap();
    for i in 0..4u64 {
        sim.play(&SeqPlan {
            proposers: vec![(i as usize) % 3, ((i + 1) as usize) % 3],
            values: vec![i, i + 1],
            choices: vec![Choice::Confirm(0); 3],
            passes: 0,
            silent_wait_ms: 0,
        })
        .unwrap();
    }
    let all = sim.inputs_all();
    let with = verify(&Tally, &all, &config()).unwrap();
    assert_eq!(with.committed_hashes(), sim.committed_hashes());
    assert_eq!(with.engaged.len(), 3);
    for c in &with.committed {
        assert_eq!(c.witnessed, (2, 3), "seq {}", c.link.payload.seq);
    }
    let mut without = all;
    without.receipts.clear();
    let bare = verify(&Tally, &without, &config()).unwrap();
    assert_eq!(bare.committed_hashes(), with.committed_hashes());
    assert_eq!(bare.status, with.status);
    for c in &bare.committed {
        assert_eq!(c.witnessed, (0, 3));
    }
}

/// §6.4 / §9.2 step 3d: a withheld vote revealed late never changes a link with a committed
/// successor. Under `q = 2, N = 3`, Carol confirms Alice's link (QC), the chain moves on, and
/// then Carol also confirms Bob's competing link from the same round — a second QC in that
/// round. With the successor in view the seq is history and the late vote is an anomaly;
/// without it, two QCs in one round is collective equivocation and the fold stops.
#[test]
fn history_is_immutable() {
    let mut sim = Sim::new(Tally, 3, 0, &["chess.example"], 2);
    sim.bootstrap().unwrap();
    let l1 = sim
        .propose(0, "add", serde_json::json!({ "n": 1 }))
        .unwrap();
    let l2 = sim
        .propose(1, "add", serde_json::json!({ "n": 2 }))
        .unwrap();
    sim.confirm(2, l1).unwrap();
    assert_eq!(
        sim.committed_hashes().len(),
        2,
        "seq 1 committed on Alice's link"
    );
    assert_eq!(sim.committed_hashes()[1], l1);
    let before = sim.inputs_all();

    let l3 = sim
        .propose(0, "add", serde_json::json!({ "n": 3 }))
        .unwrap();
    sim.confirm(1, l3).unwrap();
    assert_eq!(sim.committed_hashes().len(), 3);

    // Carol's late second vote at seq 1, round 0, for Bob's link.
    let l2_state = {
        let after = sim.inputs_all();
        let v = verify(&Tally, &after, &config()).unwrap();
        assert_eq!(v.committed_hashes()[1], l1);
        // Bob's link is not committed, so its state is not in the verdict; recompute it.
        let mut s = v.committed[0].link.payload.state.clone();
        for link in after
            .links
            .iter()
            .filter_map(|b| pubky_mayfly::fold::decode_link(b.clone()).ok())
        {
            if link.hash == l2 {
                s = link.payload.state.clone();
            }
        }
        s
    };
    let forged = sim.forge_confirmation(2, 1, Some(0), l2, &l2_state);
    let after = sim.inputs_all();
    let forged_bytes = after
        .confirms
        .iter()
        .find(|b| Hash::of(b) == forged)
        .cloned()
        .expect("forged confirmation is in Carol's folder");

    let v = verify(&Tally, &after, &config()).unwrap();
    assert_eq!(v.committed_hashes(), sim.committed_hashes());
    assert_eq!(v.committed[1].link.hash, l1);
    assert!(v.committed[1].is_final);
    assert!(v
        .anomalies
        .iter()
        .any(|a| a.kind == AnomalyKind::LateVote && a.seq == Some(1)));
    assert!(v
        .anomalies
        .iter()
        .any(|a| a.kind == AnomalyKind::Equivocation
            && a.against.as_deref() == Some(&sim.parties[2].kid())));

    let mut headless = before;
    headless.confirms.push(forged_bytes);
    match verify(&Tally, &headless, &config()) {
        Err(Error::CollectiveEquivocation { seq: 1, .. }) => {}
        other => panic!("expected collective equivocation at seq 1, got {other:?}"),
    }
}

/// §9.1 / §9.2 step 2: one party's folder alone proves the whole chain (links embed their
/// predecessor's QC; the head's QC is mirrored), and a folder holding whole QCs is never
/// hostile.
#[test]
fn mirrored_qc_is_not_hostile() {
    let mut sim = Sim::new(Tally, 4, 1, &["chess.example", "notes.example"], 4);
    sim.bootstrap().unwrap();
    for i in 0..3u64 {
        sim.play(&SeqPlan {
            proposers: vec![(i as usize) % 4, ((i + 2) as usize) % 4],
            values: vec![i, 7],
            choices: vec![
                Choice::Confirm(1),
                Choice::Confirm(0),
                Choice::Reject(0),
                Choice::Confirm(1),
            ],
            passes: 1,
            silent_wait_ms: 0,
        })
        .unwrap();
    }
    let full = verify(&Tally, &sim.inputs_all(), &config()).unwrap();
    assert_eq!(full.committed_hashes(), sim.committed_hashes());
    assert!(full.anomalies.is_empty(), "{:?}", full.anomalies);
    for folder in sim.party_folders() {
        let alone = verify(&Tally, &sim.inputs_from(&[folder.as_str()]), &config()).unwrap();
        assert_eq!(
            alone.committed_hashes(),
            full.committed_hashes(),
            "{folder}"
        );
        assert_eq!(alone.status, Status::Ongoing);
        assert!(!alone
            .anomalies
            .iter()
            .any(|a| a.kind == AnomalyKind::HostileSource));
    }
}

/// The fold refuses inputs with no valid genesis, and needs the chain named when two are valid.
#[test]
fn fold_needs_exactly_one_genesis() {
    assert!(matches!(
        verify(&Tally, &Inputs::default(), &config()),
        Err(Error::NoChain(_))
    ));
    let mut a = Sim::new(Tally, 2, 0, &[], 2);
    a.bootstrap().unwrap();
    let mut b = Sim::new(Tally, 2, 0, &[], 2);
    b.bootstrap().unwrap();
    let mut both = a.inputs_all();
    let other = b.inputs_all();
    both.links.extend(other.links);
    both.confirms.extend(other.confirms);
    both.chain = None;
    assert!(matches!(
        verify(&Tally, &both, &config()),
        Err(Error::NoChain(_))
    ));
    both.chain = a.chain.clone();
    let v = verify(&Tally, &both, &config()).unwrap();
    assert_eq!(v.committed_hashes(), a.committed_hashes());
}

// ─── Recover (§6.7) ───────────────────────────────────────────────────────────────────────────

fn single(proposer: usize, n: usize, value: u64) -> SeqPlan {
    SeqPlan::single(proposer, n, value)
}

/// §6.7 / §9.2 step 3f: an old-key reject voids a recover while it is the provisional head,
/// whatever votes it gathered; once a successor has embedded its QC the recover is history and
/// the veto is an anomaly against the old key.
#[test]
fn recover_veto_window() {
    let mut sim = Sim::new(Tally, 3, 0, &["chess.example"], 3);
    sim.bootstrap().unwrap();
    sim.play(&single(0, 3, 1)).unwrap();
    let before = sim.committed_hashes();
    let old_key = sim.parties[2].client.clone();
    let old_kid = sim.parties[2].kid();
    let seq = sim.open.seq;

    let r = sim.propose_recover(2, "notes.example").unwrap();
    sim.confirm(0, r).unwrap();
    sim.confirm(1, r).unwrap();
    assert_eq!(sim.committed_hashes().last(), Some(&r));
    let new_kid = sim.parties[2].kid();
    assert_ne!(new_kid, old_kid);

    let v = verify(&Tally, &sim.inputs_all(), &config()).unwrap();
    assert_eq!(v.committed_hashes(), sim.committed_hashes());
    assert_eq!(v.recoveries.len(), 1);
    assert_eq!(v.recoveries[0].party, 2);
    assert!(
        matches!(v.recoveries[0].delay, Verdict::Asserted { .. }),
        "no witnesses"
    );
    let seat = v
        .seats
        .iter()
        .find(|s| s.pubky == sim.parties[2].pubky())
        .unwrap();
    assert_eq!(seat.kid, new_kid);
    assert_eq!(seat.client_id, "notes.example");
    assert_eq!(seat.paths.len(), 2);

    // The old key says no while the recover is the head: void.
    let veto = sim.forge_reject(2, &old_key, seq, 0, Some(r));
    let v = verify(&Tally, &sim.inputs_all(), &config()).unwrap();
    assert_eq!(
        v.committed_hashes(),
        before,
        "a vetoed provisional recover is not committed"
    );
    assert!(v.recoveries.is_empty());
    assert!(v
        .anomalies
        .iter()
        .any(|a| a.kind == AnomalyKind::VetoedRecover
            && a.against.as_deref() == Some(new_kid.as_str())
            && a.evidence.contains(&veto)));
    assert!(matches!(v.status, Status::Stalled { seq: s, .. } if s == seq));
    let seat = v
        .seats
        .iter()
        .find(|s| s.pubky == sim.parties[2].pubky())
        .unwrap();
    assert_eq!(seat.kid, old_kid, "the seat did not change hands");

    // A successor built on the recover makes it history; the veto is now late.
    sim.play(&single(0, 3, 2)).unwrap();
    let v = verify(&Tally, &sim.inputs_all(), &config()).unwrap();
    assert_eq!(v.committed_hashes(), sim.committed_hashes());
    assert!(v.committed.iter().any(|c| c.link.hash == r && c.is_final));
    assert_eq!(v.recoveries.len(), 1);
    assert!(!v
        .anomalies
        .iter()
        .any(|a| a.kind == AnomalyKind::VetoedRecover));
    assert!(v.anomalies.iter().any(|a| a.kind == AnomalyKind::LateVeto
        && a.against.as_deref() == Some(old_kid.as_str())
        && a.evidence.contains(&veto)));
}

/// §6.7 / §9.2 step 3f: `recovery_delay_ms` is a validity rule decided by the witness quorum.
/// Confirmed inside the delay under a quorum's receipts, a recover is invalid and its
/// confirmations are anomalies; without receipts it stands, labelled asserted; confirmed after
/// the delay it is adjudicated honoured.
#[test]
fn recover_delay_by_witness_quorum() {
    let premature = |k: usize| {
        let mut sim = Sim::new(Tally, 3, k, &["chess.example"], 3);
        sim.bootstrap().unwrap();
        sim.play(&single(0, 3, 1)).unwrap();
        let before = sim.committed_hashes();
        let r = sim.propose_recover(2, "notes.example").unwrap();
        sim.witnesses_observe();
        sim.tick(60_000);
        sim.confirm(0, r).unwrap();
        sim.witnesses_observe();
        sim.confirm(1, r).unwrap();
        sim.witnesses_observe();
        (sim, before, r)
    };

    let (sim, before, r) = premature(3);
    let all = sim.inputs_all();
    let v = verify(&Tally, &all, &config()).unwrap();
    assert_eq!(v.committed_hashes(), before, "premature recover is invalid");
    assert!(v.recoveries.is_empty());
    let confirmers: Vec<String> = vec![sim.parties[0].kid(), sim.parties[1].kid()];
    for kid in &confirmers {
        assert!(v
            .anomalies
            .iter()
            .any(|a| a.kind == AnomalyKind::PrematureRecover
                && a.against.as_deref() == Some(kid.as_str())
                && a.evidence.contains(&r)));
    }
    // Without receipts nobody can prove elapsed time: the recover stands, asserted.
    let mut bare = all.clone();
    bare.receipts.clear();
    let v = verify(&Tally, &bare, &config()).unwrap();
    assert_eq!(v.committed_hashes().last(), Some(&r));
    assert!(matches!(v.recoveries[0].delay, Verdict::Asserted { .. }));
    // One witness of three decides nothing.
    let one = sim.witnesses[0].folder();
    let mut some = bare.clone();
    some.receipts = sim.inputs_from(&[one.as_str()]).receipts;
    let v = verify(&Tally, &some, &config()).unwrap();
    assert_eq!(v.committed_hashes().last(), Some(&r));
    assert!(matches!(
        v.recoveries[0].delay,
        Verdict::Asserted {
            yes: 0,
            no: 1,
            silent: 2
        }
    ));

    // Honoured.
    let mut sim = Sim::new(Tally, 3, 3, &["chess.example"], 3);
    sim.bootstrap().unwrap();
    sim.play(&single(0, 3, 1)).unwrap();
    let r = sim.propose_recover(2, "notes.example").unwrap();
    sim.witnesses_observe();
    sim.tick(pubky_mayfly::genesis::MIN_RECOVERY_DELAY_MS + 60_000);
    sim.confirm(0, r).unwrap();
    sim.witnesses_observe();
    sim.confirm(1, r).unwrap();
    sim.witnesses_observe();
    let v = verify(&Tally, &sim.inputs_all(), &config()).unwrap();
    assert_eq!(v.committed_hashes(), sim.committed_hashes());
    assert_eq!(v.recoveries[0].delay, Verdict::Yes);
    assert!(v.anomalies.is_empty(), "{:?}", v.anomalies);
}

/// §7: a party's records live under the folder they declared; a `recover` moves it, and the
/// verifier reads every folder a party has declared, in order.
#[test]
fn folders_follow_declared_paths() {
    let mut sim = Sim::new(Tally, 3, 0, &["chess.example", "notes.example"], 3);
    sim.bootstrap().unwrap();
    sim.play(&single(2, 3, 1)).unwrap();
    let old_folder = sim.parties[2].folder();
    let r = sim.propose_recover(2, "recovered.example").unwrap();
    sim.confirm(0, r).unwrap();
    sim.confirm(1, r).unwrap();
    let new_folder = sim.parties[2].folder();
    assert_ne!(old_folder, new_folder);
    sim.play(&single(2, 3, 2)).unwrap();
    sim.play(&single(0, 3, 3)).unwrap();

    let full = verify(&Tally, &sim.inputs_all(), &config()).unwrap();
    assert_eq!(full.committed_hashes(), sim.committed_hashes());
    assert!(full.anomalies.is_empty(), "{:?}", full.anomalies);
    let seat = full
        .seats
        .iter()
        .find(|s| s.pubky == sim.parties[2].pubky())
        .unwrap();
    assert_eq!(
        seat.paths,
        vec![
            "/pub/chess.example/mayfly/".to_string(),
            "/pub/recovered.example/mayfly/".to_string(),
        ]
    );

    // Every currently declared folder together proves the chain …
    let current: Vec<String> = sim.party_folders();
    let refs: Vec<&str> = current.iter().map(String::as_str).collect();
    let v = verify(&Tally, &sim.inputs_from(&refs), &config()).unwrap();
    assert_eq!(v.committed_hashes(), full.committed_hashes());
    // … and so do the recovered party's two folders alone, old then new.
    let v = verify(
        &Tally,
        &sim.inputs_from(&[old_folder.as_str(), new_folder.as_str()]),
        &config(),
    )
    .unwrap();
    assert_eq!(v.committed_hashes(), full.committed_hashes());
    // The new folder alone predates nothing: the history before the recover is not there.
    assert!(verify(&Tally, &sim.inputs_from(&[new_folder.as_str()]), &config()).is_err());
}

// ─── Abandoned close (§6.8, §11.3) ────────────────────────────────────────────────────────────

/// Alice proposes, Bob confirms, Carol says nothing for `wait_ms`; Alice closes naming Carol
/// and Bob confirms the close. Witnesses observe everything.
fn abandon_carol(k: usize, wait_ms: u64) -> (Sim<Tally>, Hash) {
    let mut sim = Sim::new(Tally, 3, k, &["chess.example"], 3);
    sim.bootstrap().unwrap();
    sim.play(&single(0, 3, 1)).unwrap();
    let l = sim
        .propose(0, "add", serde_json::json!({ "n": 2 }))
        .unwrap();
    sim.tick(1_000);
    sim.witnesses_observe();
    sim.confirm(1, l).unwrap();
    sim.tick(1_000);
    sim.witnesses_observe();
    sim.tick(wait_ms);
    let close = sim.propose_abandoned(0, &[2]).unwrap();
    sim.tick(1_000);
    sim.witnesses_observe();
    sim.confirm_abandoned(1, close).unwrap();
    sim.tick(1_000);
    sim.witnesses_observe();
    (sim, close)
}

const HOUR_MS: u64 = 3_600_000;

/// How a verdict on an abandoned close reads.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CloseReading {
    Ended,
    Dropped,
    Provisional,
}

fn read_close(v: &pubky_mayfly::fold::Verdict, close: Hash) -> CloseReading {
    match &v.status {
        Status::Abandoned { subjects, .. } => {
            assert_eq!(subjects, &vec![2]);
            assert_eq!(v.committed_hashes().last(), Some(&close));
            CloseReading::Ended
        }
        Status::Paused { .. } => CloseReading::Provisional,
        Status::Stalled { .. } => {
            assert!(v
                .anomalies
                .iter()
                .any(|a| a.kind == AnomalyKind::VoidClose && a.evidence.contains(&close)));
            CloseReading::Dropped
        }
        other => panic!("unexpected {other:?}"),
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, .. ProptestConfig::default() })]

    /// §6.8: only an adjudicated close is final; an asserted or contested close pauses. Two
    /// verifiers with different receipt sets never reach contradictory finals: with every
    /// receipt the close is adjudicated one way, and any subset yields the same final or a
    /// provisional verdict.
    #[test]
    fn close_verdicts_never_contradict_when_final(
        k in 1usize..=3,
        wait in prop_oneof![Just(HOUR_MS), Just(23 * HOUR_MS), Just(25 * HOUR_MS), Just(72 * HOUR_MS)],
        mask_a in any::<u64>(),
        mask_b in any::<u64>(),
    ) {
        let (sim, close) = abandon_carol(k, wait);
        let all = sim.inputs_all();
        let full = verify(&Tally, &all, &config()).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let expected = if wait > 24 * HOUR_MS { CloseReading::Ended } else { CloseReading::Dropped };
        prop_assert_eq!(read_close(&full, close), expected.clone());

        let mut bare = all.clone();
        bare.receipts.clear();
        let v = verify(&Tally, &bare, &config()).map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(read_close(&v, close), CloseReading::Provisional);
        prop_assert_eq!(&v.status, &Status::Paused { seq: 2, close: pubky_mayfly::close::CloseState::Asserted });

        for mask in [mask_a, mask_b] {
            let mut partial = all.clone();
            partial.receipts = partial
                .receipts
                .into_iter()
                .enumerate()
                .filter(|(i, _)| keep(mask, *i))
                .map(|(_, b)| b)
                .collect();
            let v = verify(&Tally, &partial, &config()).map_err(|e| TestCaseError::fail(e.to_string()))?;
            let reading = read_close(&v, close);
            prop_assert!(
                reading == CloseReading::Provisional || reading == expected,
                "receipts subset reached {:?} where everything reaches {:?}", reading, expected
            );
            // Whatever the close, the committed prefix is the same.
            prop_assert_eq!(&v.committed_hashes()[..2], &full.committed_hashes()[..2]);
        }
    }
}

/// §6.8: presence is decided from files, never from receipts. A subject with any record at
/// the seq contests an asserted close; with a witness quorum, a subject who owed nothing
/// cannot be closed out, and a subject who answered before the close is not silent.
#[test]
fn presence_from_files() {
    // Carol proposes something nobody confirms, then Alice closes naming her: contested.
    let mut sim = Sim::new(Tally, 3, 0, &["chess.example"], 3);
    sim.bootstrap().unwrap();
    sim.propose(2, "add", serde_json::json!({ "n": 9 }))
        .unwrap();
    let close = sim.propose_abandoned(0, &[2]).unwrap();
    sim.confirm_abandoned(1, close).unwrap();
    let v = verify(&Tally, &sim.inputs_all(), &config()).unwrap();
    assert_eq!(
        v.status,
        Status::Paused {
            seq: 1,
            close: pubky_mayfly::close::CloseState::Contested { present: vec![2] }
        }
    );
    assert_eq!(v.committed_hashes(), sim.committed_hashes());

    // Same, but with a quorum of receipts: Carol owed nobody a vote, so the close is invalid.
    let mut sim = Sim::new(Tally, 3, 3, &["chess.example"], 3);
    sim.bootstrap().unwrap();
    sim.propose(2, "add", serde_json::json!({ "n": 9 }))
        .unwrap();
    sim.witnesses_observe();
    sim.tick(30 * HOUR_MS);
    let close = sim.propose_abandoned(0, &[2]).unwrap();
    sim.witnesses_observe();
    sim.confirm_abandoned(1, close).unwrap();
    sim.witnesses_observe();
    let v = verify(&Tally, &sim.inputs_all(), &config()).unwrap();
    assert_eq!(read_close(&v, close), CloseReading::Dropped);

    // Carol answers Alice's proposal (with a reject) before the close: she was not silent.
    let mut sim = Sim::new(Tally, 3, 3, &["chess.example"], 3);
    sim.bootstrap().unwrap();
    let l = sim
        .propose(0, "add", serde_json::json!({ "n": 1 }))
        .unwrap();
    sim.witnesses_observe();
    sim.tick(30 * HOUR_MS);
    sim.reject(2, l).unwrap();
    sim.witnesses_observe();
    sim.tick(HOUR_MS);
    let close = sim.propose_abandoned(0, &[2]).unwrap();
    sim.witnesses_observe();
    sim.confirm_abandoned(1, close).unwrap();
    sim.witnesses_observe();
    let v = verify(&Tally, &sim.inputs_all(), &config()).unwrap();
    assert_eq!(read_close(&v, close), CloseReading::Dropped);
    // Her reject of the sole valid proposal is obstruction, and the files say so.
    assert!(v
        .anomalies
        .iter()
        .any(|a| a.kind == AnomalyKind::Obstruction
            && a.against.as_deref() == Some(sim.parties[2].kid().as_str())));
}

// ─── Reveals (§6.6) ───────────────────────────────────────────────────────────────────────────

/// §6.6: when the rules want randomness, each party after the initiator reveals its nonce in
/// genesis order, the verifier checks `BLAKE3(nonce)` against the genesis `commit`, and the
/// rules initialise only when the last reveal commits. A wrong nonce is not a candidate; a
/// reveal on rules that want none is not a candidate.
#[test]
fn reveals_seat_the_rules() {
    let mut sim = Sim::new(RandomTally, 3, 0, &["chess.example"], 3);
    sim.bootstrap().unwrap();
    assert!(
        sim.head().unwrap().state.is_none(),
        "no state before the reveals"
    );
    assert!(sim
        .propose(0, "add", serde_json::json!({ "n": 1 }))
        .is_err());
    assert!(sim.propose_reveal(2).is_err(), "party 1 reveals first");
    let r1 = sim.propose_reveal(1).unwrap();
    sim.confirm(0, r1).unwrap();
    sim.confirm(2, r1).unwrap();
    assert!(sim.head().unwrap().state.is_none());
    let r2 = sim.propose_reveal(2).unwrap();
    sim.confirm(0, r2).unwrap();
    sim.confirm(1, r2).unwrap();
    let seed = sim.head().unwrap().state.as_ref().unwrap().seed.clone();
    assert!(!seed.is_empty(), "the rules initialised from the reveals");
    sim.play(&single(1, 3, 5)).unwrap();
    let v = verify(&RandomTally, &sim.inputs_all(), &config()).unwrap();
    assert_eq!(v.committed_hashes(), sim.committed_hashes());
    assert_eq!(v.status, Status::Ongoing);
    assert!(v.anomalies.is_empty(), "{:?}", v.anomalies);

    // A nonce that does not hash to the commit is not a reveal.
    let mut sim = Sim::new(RandomTally, 3, 0, &["chess.example"], 3);
    sim.bootstrap().unwrap();
    sim.parties[1].nonce = [7; 16];
    let bad = sim.propose_reveal(1).unwrap();
    sim.confirm(0, bad).unwrap();
    sim.confirm(2, bad).unwrap();
    let v = verify(&RandomTally, &sim.inputs_all(), &config()).unwrap();
    assert_eq!(
        v.committed_hashes().len(),
        1,
        "the false reveal did not commit"
    );
    assert!(matches!(v.status, Status::Stalled { seq: 1, .. }));

    // Rules that want no randomness have nothing to reveal.
    let mut sim = Sim::new(Tally, 2, 0, &["chess.example"], 2);
    sim.bootstrap().unwrap();
    let r = sim.propose_reveal(1).unwrap();
    sim.confirm(0, r).unwrap();
    let v = verify(&Tally, &sim.inputs_all(), &config()).unwrap();
    assert_eq!(v.committed_hashes().len(), 1);
}

// ─── Witness rotation (§11.2) ─────────────────────────────────────────────────────────────────

/// §11.2: a `witnesses` link seats the added witnesses (their `engage.jws` verified in the
/// fold, so their receipts become usable) and unseats the removed ones from the next seq; it
/// commits only with every party's confirmation, whatever `confirm_quorum` says.
#[test]
fn witnesses_link_rotates_the_engaged_set() {
    let mut sim = Sim::new(Tally, 3, 1, &["chess.example"], 2);
    sim.witnesses[0].behaviour = WitnessBehaviour::Dark;
    sim.bootstrap().unwrap();
    sim.play(&single(0, 3, 1)).unwrap();
    let v = verify(&Tally, &sim.inputs_all(), &config()).unwrap();
    assert_eq!(
        v.head().unwrap().witnessed,
        (0, 1),
        "the dark witness receipts nothing"
    );

    let w1 = sim.add_witness(WitnessBehaviour::Honest).unwrap();
    let before = sim.committed_hashes();
    let link = sim.propose_witnesses(0, &[w1], &[0]).unwrap();
    sim.confirm(1, link).unwrap();
    // q = 2 would commit an ordinary link here; a witnesses link needs everyone.
    assert_eq!(sim.committed_hashes(), before);
    let v = verify(&Tally, &sim.inputs_all(), &config()).unwrap();
    assert_eq!(v.committed_hashes(), before);
    assert!(matches!(v.status, Status::Stalled { .. }));
    sim.confirm(2, link).unwrap();
    assert_eq!(sim.committed_hashes().last(), Some(&link));
    sim.witnesses_observe();

    sim.play(&single(1, 3, 2)).unwrap();
    sim.play(&single(2, 3, 3)).unwrap();
    let v = verify(&Tally, &sim.inputs_all(), &config()).unwrap();
    assert_eq!(v.committed_hashes(), sim.committed_hashes());
    assert_eq!(v.engaged.len(), 1);
    assert_eq!(v.engaged[0].pubky, sim.witnesses[w1].pubky());
    assert_eq!(
        v.head().unwrap().witnessed,
        (1, 1),
        "the new witness counts from the next seq"
    );
    assert!(v.anomalies.is_empty(), "{:?}", v.anomalies);
}

// ─── Grant windows (§9.2 step 4) ──────────────────────────────────────────────────────────────

/// §9.2 step 4: a record a witness quorum observed after its Grant's `exp`, or after a receipted
/// revocation of its key, is invalid while provisional and an anomaly once history; without a
/// quorum of receipts no window check is possible.
#[test]
fn grant_windows_by_witness_quorum() {
    const DAY_S: u64 = 86_400;

    // Expiry, under unanimity: every QC after Carol's `exp` needs her expired key, so once her
    // latest vote falls, the seq before it is no longer history and falls too — the whole tail
    // after expiry is outside the window (§6.7: rekey *before* `exp`).
    let mut sim = Sim::new(Tally, 3, 3, &["chess.example"], 3);
    sim.reissue_grant(2, DAY_S);
    sim.bootstrap().unwrap();
    sim.play(&single(0, 3, 1)).unwrap();
    let before = sim.committed_hashes();
    sim.tick(2 * DAY_S * 1000);
    sim.play(&single(0, 3, 2)).unwrap();
    let all = sim.inputs_all();
    let v = verify(&Tally, &all, &config()).unwrap();
    assert_eq!(
        v.committed_hashes(),
        before,
        "Carol's expired confirmation is invalid"
    );
    assert!(matches!(v.status, Status::Stalled { seq: 2, .. }));
    let kid2 = sim.parties[2].kid();
    assert!(
        v.anomalies
            .iter()
            .all(|a| a.kind == AnomalyKind::GrantWindow
                && a.against.as_deref() == Some(kid2.as_str())),
        "{:?}",
        v.anomalies
    );
    assert!(!v.anomalies.is_empty());
    let mut bare = all.clone();
    bare.receipts.clear();
    let v = verify(&Tally, &bare, &config()).unwrap();
    assert_eq!(
        v.committed_hashes(),
        sim.committed_hashes(),
        "no receipts, no window check"
    );
    assert!(v.anomalies.is_empty());
    sim.play(&single(0, 3, 3)).unwrap();
    let v = verify(&Tally, &sim.inputs_all(), &config()).unwrap();
    assert_eq!(v.committed_hashes(), before);
    assert!(matches!(v.status, Status::Stalled { seq: 2, .. }));
    assert!(v
        .anomalies
        .iter()
        .any(|a| a.kind == AnomalyKind::GrantWindow && a.seq == Some(3)));

    // Expiry, under q = 2: Carol's expired link is invalid while it is the head, but once a
    // successor whose QC does not need her embeds it, it is history — an anomaly against her
    // and against the confirmer who accepted it, never a rewrite.
    let mut sim = Sim::new(Tally, 3, 3, &["chess.example"], 2);
    sim.reissue_grant(2, DAY_S);
    sim.bootstrap().unwrap();
    sim.play(&single(0, 3, 1)).unwrap();
    let before = sim.committed_hashes();
    sim.tick(2 * DAY_S * 1000);
    sim.play(&single(2, 3, 2)).unwrap();
    let carols = *sim.committed_hashes().last().unwrap();
    let v = verify(&Tally, &sim.inputs_all(), &config()).unwrap();
    assert_eq!(
        v.committed_hashes(),
        before,
        "Carol's expired link is invalid as the head"
    );
    sim.play(&single(0, 3, 3)).unwrap();
    let v = verify(&Tally, &sim.inputs_all(), &config()).unwrap();
    assert_eq!(
        v.committed_hashes(),
        sim.committed_hashes(),
        "history is not rewritten"
    );
    assert_eq!(v.status, Status::Ongoing);
    let kid2 = sim.parties[2].kid();
    let kid0 = sim.parties[0].kid();
    assert!(v
        .anomalies
        .iter()
        .any(|a| a.kind == AnomalyKind::GrantWindow
            && a.against.as_deref() == Some(kid2.as_str())
            && a.evidence == vec![carols]));
    assert!(
        v.anomalies
            .iter()
            .any(|a| a.kind == AnomalyKind::GrantWindow
                && a.against.as_deref() == Some(kid0.as_str())
                && a.evidence == vec![carols]),
        "the confirmer who accepted an expired Grant: {:?}",
        v.anomalies
    );

    // Revocation.
    let mut sim = Sim::new(Tally, 3, 3, &["chess.example"], 3);
    sim.bootstrap().unwrap();
    sim.play(&single(0, 3, 1)).unwrap();
    let before = sim.committed_hashes();
    let revocation = sim.revoke_key(2);
    sim.tick(1_000);
    sim.witnesses_observe();
    sim.tick(60_000);
    sim.play(&single(0, 3, 2)).unwrap();
    let all = sim.inputs_all();
    assert!(all.revocations.iter().any(|b| Hash::of(b) == revocation));
    let v = verify(&Tally, &all, &config()).unwrap();
    assert_eq!(
        v.committed_hashes(),
        before,
        "a record by a revoked key, observed later, is invalid"
    );
    let kid2 = sim.parties[2].kid();
    assert!(
        v.anomalies
            .iter()
            .any(|a| a.kind == AnomalyKind::GrantWindow
                && a.against.as_deref() == Some(kid2.as_str()))
    );
    let mut bare = all;
    bare.receipts.clear();
    let v = verify(&Tally, &bare, &config()).unwrap();
    assert_eq!(v.committed_hashes(), sim.committed_hashes());
}

// ─── Skips (§6.3, §6.4) ───────────────────────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, .. ProptestConfig::default() })]

    /// §6.4: a skip pushes a seq at most `N − 1` rounds. One party walks away under
    /// `q = N − 1`; when rotation designates them, the others skip them, once each per seq;
    /// a second skip is not a vote (`InvalidSkip`); and with a witness, an immediate skip is
    /// premature (§11.3), reported, never a validity question — while a skip after `think_ms`
    /// is not.
    #[test]
    fn skips_are_bounded(n in 4usize..=5, walker in 0usize..5, k in 0usize..=1, patient in any::<bool>()) {
        let walker = walker % n;
        let mut sim = Sim::new(Tally, n, k, &["chess.example"], n - 1);
        for p in 0..n {
            sim.parties[p].behaviour = if p == walker { Behaviour::WalksAwayAt(1) } else { Behaviour::SkipsEarly };
        }
        sim.bootstrap().map_err(|e| TestCaseError::fail(e.to_string()))?;
        let seq = sim.open.seq;
        // Two present parties propose, the other present parties refuse both: round 0 dies
        // without the walker's vote (needs N >= 4 under q = N − 1).
        let present: Vec<usize> = (0..n).filter(|p| *p != walker).collect();
        let rounds = sim
            .play(&SeqPlan {
                proposers: vec![present[0], present[1]],
                values: vec![1, 2],
                choices: vec![Choice::Reject(0); n],
                passes: 0,
                silent_wait_ms: if patient { 25 * HOUR_MS } else { 0 },
            })
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert!(rounds <= 1 + n as u32, "rounds {rounds}");

        let v = verify(&Tally, &sim.inputs_all(), &config()).map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(v.committed_hashes(), sim.committed_hashes());
        prop_assert!(v.head().unwrap().link.payload.round <= n as u32);
        let skips: Vec<(u64, u32, usize)> = sim.skip_log.iter().copied().filter(|(s, _, _)| *s == seq).collect();
        prop_assert!(skips.len() < n, "at most N − 1 skips per seq");
        let premature = v.anomalies.iter().filter(|a| a.kind == AnomalyKind::PrematureSkip).count();
        prop_assert!(v.anomalies.iter().all(|a| a.kind == AnomalyKind::PrematureSkip), "{:?}", v.anomalies);
        if k == 1 && !patient {
            prop_assert_eq!(premature, skips.len(), "{:?}", v.anomalies);
        } else {
            prop_assert_eq!(premature, 0, "{:?}", v.anomalies);
        }

        // A second skip by a party who already skipped at that seq is not a vote.
        if let Some(&(_, round, s)) = skips.first() {
            let signer = sim.parties[s].client.clone();
            sim.forge_reject(s, &signer, seq, round, None);
            let v2 = verify(&Tally, &sim.inputs_all(), &config()).map_err(|e| TestCaseError::fail(e.to_string()))?;
            prop_assert_eq!(v2.committed_hashes(), v.committed_hashes());
            prop_assert_eq!(&v2.status, &v.status);
            let kid = sim.parties[s].kid();
            prop_assert!(
                v2.anomalies.iter().any(|a| a.kind == AnomalyKind::InvalidSkip && a.against.as_deref() == Some(kid.as_str())),
                "{:?}", v2.anomalies
            );
        }
    }
}
