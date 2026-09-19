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
use pubky_mayfly::sim::{Choice, SeqPlan, Sim, Tally, WitnessBehaviour};
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
        // Deleted rejects may make a round look unjustified; that is a report, not an input.
        prop_assert!(v
            .anomalies
            .iter()
            .all(|a| a.kind == AnomalyKind::UnjustifiedRound), "{:?}", v.anomalies);
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

// ─── Waiting on the rest of the fold ─────────────────────────────────────────────────────────
//
// Each of these is one sentence from the specification; the message says which section it
// pins and what it is waiting for.

#[test]
#[ignore = "§6.4: a skip pushes a seq at most N − 1 rounds — needs sim Behaviour::SkipsEarly"]
fn skips_are_bounded() {}

#[test]
#[ignore = "§6.8: only an adjudicated close is final; asserted/contested pauses — needs the stopwatch (§11.3) in step 3h"]
fn close_verdicts_never_contradict_when_final() {}

#[test]
#[ignore = "§6.8: presence is decided from files — a subject record at the seq always contests an asserted close; needs sim propose_abandoned"]
fn presence_from_files() {}

#[test]
#[ignore = "§6.7: old-key veto voids a provisional recover and is an anomaly after it is history — needs recover in step 3a/3f"]
fn recover_veto_window() {}

#[test]
#[ignore = "§7: a recover moves a party's folder and the fold follows — needs recover in step 3a/3f"]
fn folders_follow_declared_paths() {}
