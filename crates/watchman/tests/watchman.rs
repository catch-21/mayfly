//! Phase 5 (§16.2) over the in-memory store: a watchman engages, receipts a three-party chain
//! in causal order, and the parties' folds report every link *witnessed 1/1*, embed its
//! receipts and mirror them; an abandoned close is merely asserted until the watchman's
//! receipts adjudicate it; a double vote and a misnamed mirror are receipted as inconsistent;
//! the `mirror` tier keeps byte copies; a lapsed engagement receipts nothing.
//!
//! `tests/testnet.rs` runs the first flow over `PubkyStore` against an `EphemeralTestnet`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pubky_common::crypto::Keypair;
use serde_json::json;

use pubky_mayfly::close::CloseState;
use pubky_mayfly::fold::Status;
use pubky_mayfly::hash::Hash;
use pubky_mayfly::record::{CloseReason, Confirmation, Link, Signed};
use pubky_mayfly::sim::{Storage, Tally};
use pubky_mayfly::{typ, PROTOCOL_VERSION};
use pubky_mayfly_client::chain::verify_from;
use pubky_mayfly_client::layout::Folder;
use pubky_mayfly_client::{ChainClient, GenesisSpec, LocalSigner, MemoryStore, Signer, Store};
use pubky_mayfly_watchman::{Credit, Declined, Operator, Terms, Watchman};

type Client = ChainClient<Tally, MemoryStore, LocalSigner>;
type Dog = Watchman<MemoryStore, LocalSigner>;

const NOW_S: u64 = 1_757_779_812;
const NOW_MS: u64 = NOW_S * 1000;
const TWO_YEARS: u64 = 2 * 365 * 24 * 3600;
const HOUR_MS: u64 = 3_600_000;

struct Actor {
    signer: LocalSigner,
    store: MemoryStore,
}

fn actor(shared: &Arc<Mutex<Storage>>, client_id: &str) -> Actor {
    let identity = Keypair::random();
    let signer = LocalSigner::mint(&identity, client_id, NOW_S, TWO_YEARS);
    let store = MemoryStore::new(identity.public_key().z32(), Arc::clone(shared));
    Actor { signer, store }
}

/// A shared, settable clock: the parties and the watchman read the same "now".
#[derive(Clone)]
struct Clock(Arc<AtomicU64>);

impl Clock {
    fn new(ms: u64) -> Self {
        Self(Arc::new(AtomicU64::new(ms)))
    }
    fn now(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn advance(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
    fn reader(&self) -> impl Fn() -> u64 + Send + Sync + 'static {
        let c = self.clone();
        move || c.now()
    }
}

struct World {
    clients: Vec<Client>,
    dog: Dog,
    actors: Vec<Actor>,
    clock: Clock,
    shared: Arc<Mutex<Storage>>,
}

/// Alice, Bob and Carol on two apps; one watchman named at genesis. Genesis is written,
/// the watchman engages, the parties join, and everyone syncs and mirrors. Nothing has been
/// receipted yet.
async fn world(terms: impl Fn(u64) -> Terms) -> World {
    let shared = MemoryStore::shared();
    let clock = Clock::new(NOW_MS);
    let actors = vec![
        actor(&shared, "chess.example"),
        actor(&shared, "notes.example"),
        actor(&shared, "notes.example"),
    ];
    let watchman = actor(&shared, "watchman.example");
    let pubkies: Vec<String> = actors.iter().map(|a| a.signer.pubky()).collect();
    let mut spec = GenesisSpec::new(pubkies.clone()).with_apps(&[
        "chess.example",
        "notes.example",
        "notes.example",
    ]);
    spec.witnesses = vec![watchman.signer.pubky()];
    let alice = ChainClient::create(
        Tally,
        actors[0].store.clone(),
        actors[0].signer.clone(),
        spec,
    )
    .await
    .unwrap()
    .with_clock(clock.reader());
    let chain = alice.chain().clone();
    let initiator = (pubkies[0].clone(), actors[0].signer.path());

    let mut dog = Watchman::new(
        watchman.store.clone(),
        watchman.signer.clone(),
        chain.clone(),
        initiator.clone(),
        terms(NOW_S + TWO_YEARS),
    )
    .with_clock(clock.reader());
    dog.engage().await.unwrap();

    let mut clients = vec![alice];
    for a in &actors[1..] {
        let mut c = ChainClient::open(
            Tally,
            a.store.clone(),
            a.signer.clone(),
            chain.clone(),
            initiator.clone(),
        )
        .with_clock(clock.reader());
        c.sync().await.unwrap();
        c.join().await.unwrap();
        clients.push(c);
    }
    sync_all(&mut clients).await;
    for c in &clients {
        let v = c.verdict().unwrap();
        assert_eq!(v.committed_hashes().len(), 1, "genesis committed");
        assert_eq!(
            v.engaged.len(),
            1,
            "the witness was found from its /pub/ alone"
        );
        assert_eq!(v.engaged[0].kid, watchman.signer.kid());
        assert_eq!(v.committed[0].witnessed, (0, 1), "nothing receipted yet");
    }
    World {
        clients,
        dog,
        actors,
        clock,
        shared,
    }
}

async fn sync_all(clients: &mut [Client]) {
    for c in clients.iter_mut() {
        c.sync().await.unwrap();
        c.mirror().await.unwrap();
    }
}

fn committed(c: &Client) -> Vec<Hash> {
    c.verdict().unwrap().committed_hashes()
}

/// Propose by `by`, confirm by everyone else, receipt, sync — with a minute passing between
/// each step. Returns the link.
async fn append(w: &mut World, by: usize, n: u64) -> Hash {
    w.clock.advance(60_000);
    let l = w.clients[by]
        .propose("add", json!({ "n": n }))
        .await
        .unwrap();
    sync_all(&mut w.clients).await;
    w.dog.poll().await.unwrap();
    for (i, c) in w.clients.iter_mut().enumerate() {
        if i != by {
            w.clock.advance(60_000);
            c.confirm(l).await.unwrap();
        }
    }
    w.clock.advance(60_000);
    w.dog.poll().await.unwrap();
    sync_all(&mut w.clients).await;
    l
}

#[tokio::test]
async fn a_watchman_receipts_the_chain_and_every_link_is_witnessed() {
    let mut w = world(Terms::receipts).await;
    let dog_kid = w.dog.signer().kid();

    // The first sweep receipts genesis and its QC; the parties see genesis witnessed 1/1.
    let report = w.dog.poll().await.unwrap();
    assert_eq!(report.receipts, 3, "genesis and two confirmations");
    assert!(report.inconsistent.is_empty());
    sync_all(&mut w.clients).await;
    for c in &w.clients {
        assert_eq!(c.verdict().unwrap().committed[0].witnessed, (1, 1));
    }
    assert_eq!(
        w.dog.poll().await.unwrap().receipts,
        0,
        "a record is receipted once"
    );

    // Every append is witnessed once the sweep has run, and the next proposal embeds the
    // receipt of the QC-completing confirmation (§6.1).
    let l1 = append(&mut w, 0, 5).await;
    let l2 = append(&mut w, 1, 7).await;
    for c in &w.clients {
        let v = c.verdict().unwrap();
        assert_eq!(
            v.committed_hashes(),
            vec![committed(&w.clients[0])[0], l1, l2]
        );
        for k in &v.committed {
            assert_eq!(k.witnessed, (1, 1), "seq {}", k.link.payload.seq);
        }
        assert_eq!(
            v.committed[2].link.payload.receipts.len(),
            1,
            "l2 embeds l1's receipt"
        );
        assert!(v.anomalies.is_empty(), "{:?}", v.anomalies);
        assert_eq!(c.state().unwrap().unwrap().total, 12);
    }

    // Causal order (§11.3): no link was receipted before the QC of its prev, and no vote
    // before the link it votes on, even though the clock advanced between sweeps.
    for c in w.clients[0].verdict().unwrap().committed.windows(2) {
        let (prev, link) = (&c[0], &c[1]);
        let ready = prev
            .qc
            .iter()
            .map(|q| w.dog.observed_at(&q.hash).unwrap())
            .max()
            .unwrap_or(0);
        assert!(w.dog.observed_at(&link.link.hash).unwrap() >= ready);
        for q in &link.qc {
            assert!(
                w.dog.observed_at(&q.hash).unwrap() >= w.dog.observed_at(&link.link.hash).unwrap()
            );
        }
    }

    // Every party mirrored the engagement and the receipts (§7), so the timeline is readable
    // from any one party's folder even if the watchman's storage vanished.
    for c in &w.clients {
        let folder = Folder::from_path(&c.signer().path()).chain(c.chain());
        let mirrored = c
            .store()
            .list(
                c.store().me(),
                &format!("{}receipts/{dog_kid}/", folder.as_str()),
            )
            .await
            .unwrap();
        assert!(mirrored.iter().any(|l| l.path.ends_with("/engage.jws")));
        assert_eq!(
            mirrored.len(),
            1 + 3 * 2,
            "the engagement, plus a receipt per QC member of each of three committed links"
        );
    }
    {
        let mut storage = w.shared.lock().unwrap();
        let dog_folder = format!("pubky://{}{}", w.dog.store().me(), w.dog.signer().path());
        storage.folders.remove(&dog_folder);
    }
    let observer = MemoryStore::new("", Arc::clone(&w.shared));
    let report = verify_from(
        &Tally,
        &observer,
        w.clients[0].chain(),
        (
            w.clients[1].store().me().to_string(),
            w.clients[1].signer().path(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(report.verdict.committed_hashes(), committed(&w.clients[0]));
    assert_eq!(
        report.verdict.engaged.len(),
        1,
        "the engagement survives on the parties' mirrors"
    );
    for k in &report.verdict.committed {
        assert_eq!(k.witnessed, (1, 1));
    }
}

#[tokio::test]
async fn an_abandoned_close_is_asserted_until_the_watchman_adjudicates_it() {
    let mut w = world(Terms::receipts).await;
    w.dog.poll().await.unwrap();

    // Alice proposes; Bob confirms; Carol says nothing. The watchman sees the proposal now.
    let l1 = w.clients[0]
        .propose("add", json!({ "n": 3 }))
        .await
        .unwrap();
    sync_all(&mut w.clients).await;
    w.clients[1].confirm(l1).await.unwrap();
    w.dog.poll().await.unwrap();

    // A day and an hour later (`respond_ms` defaults to 24 h, §11.3) Alice closes on Carol
    // and Bob agrees. Before the watchman has receipted the close, nobody can say how long
    // Carol was silent: the close is asserted, the chain paused, nothing final.
    w.clock.advance(25 * HOUR_MS);
    let close = w.clients[0].propose_abandoned(&[2]).await.unwrap();
    sync_all(&mut w.clients[..2]).await;
    w.clients[1].confirm_abandoned(close).await.unwrap();
    sync_all(&mut w.clients[..2]).await;
    for c in &w.clients[..2] {
        let v = c.verdict().unwrap();
        assert_eq!(
            v.status,
            Status::Paused {
                seq: 1,
                close: CloseState::Asserted,
            }
        );
        assert_eq!(v.committed_hashes().len(), 1);
        assert!(!v.is_final());
    }

    // The watchman's receipts of the close and of Bob's agreement complete the quorum's view
    // (k = 1, so one witness is the quorum): Carol was silent 25 h > 24 h. Final.
    let report = w.dog.poll().await.unwrap();
    assert_eq!(report.receipts, 2, "the close and Bob's confirmation");
    sync_all(&mut w.clients[..2]).await;
    for c in &w.clients[..2] {
        let v = c.verdict().unwrap();
        assert!(
            matches!(&v.status, Status::Abandoned { subjects, .. } if *subjects == vec![2]),
            "{:?}",
            v.status
        );
        assert!(v.is_final());
        assert_eq!(v.committed_hashes().last(), Some(&close));
        assert_eq!(v.head().unwrap().witnessed, (1, 1));
    }

    // Carol turns up afterwards and confirms l1: the fold notes a late subject, nothing
    // reopens.
    w.clients[2].sync().await.unwrap();
    let _ = w.clients[2].confirm(l1).await;
    sync_all(&mut w.clients).await;
    for c in &w.clients {
        assert!(c.verdict().unwrap().is_final());
    }
}

#[tokio::test]
async fn a_double_vote_and_a_misnamed_mirror_are_receipted_as_inconsistent() {
    let mut w = world(Terms::receipts).await;
    w.dog.poll().await.unwrap();
    let l1 = append(&mut w, 0, 1).await;
    let chain = w.clients[0].chain().clone();
    let bob = &w.actors[1];
    let bob_folder = Folder::from_path(&bob.signer.path()).chain(&chain);

    // Bob confirms Carol's proposal at seq 2 and the watchman sees it; then Bob also
    // "confirms" a link that does not exist, in the same round: a second vote by one key in
    // one round (§6.4).
    let l2 = w.clients[2]
        .propose("add", json!({ "n": 2 }))
        .await
        .unwrap();
    sync_all(&mut w.clients).await;
    w.clients[1].confirm(l2).await.unwrap();
    w.dog.poll().await.unwrap();
    let phantom = Hash::of(b"a link nobody proposed");
    let forged = Confirmation {
        v: PROTOCOL_VERSION,
        chain: chain.clone(),
        seq: 2,
        round: Some(0),
        link: phantom.to_base64url(),
        kid: bob.signer.kid(),
        ts: w.clock.now(),
        state: "irrelevant".into(),
        grant: None,
        path: None,
        commit: None,
    };
    let forged_bytes =
        pubky_mayfly::record::sign(bob.signer.keypair(), typ::CONFIRM, &forged).unwrap();
    let forged_hash = Hash::of(&forged_bytes);
    bob.store
        .put(&bob_folder.confirm(2, &phantom), forged_bytes)
        .await
        .unwrap();

    // Bob also overwrites his mirror of Alice's committed l1 with a link of his own: bytes
    // that do not have the hash the name claims (§7, §11.3). At seq 3 so that this is not
    // also a double vote.
    let head = w.clients[1].verdict().unwrap().head().unwrap().clone();
    let misnamed = Link {
        v: PROTOCOL_VERSION,
        chain: chain.clone(),
        seq: 3,
        round: 0,
        prev: head.link.hash.to_base64url(),
        confirms: Vec::new(),
        receipts: Vec::new(),
        author: bob.signer.pubky(),
        kid: bob.signer.kid(),
        ts: w.clock.now(),
        kind: "add".into(),
        body: json!({ "n": 99 }),
        state: head.link.payload.state.clone(),
        grant: None,
    };
    let misnamed_bytes =
        pubky_mayfly::record::sign(bob.signer.keypair(), typ::LINK, &misnamed).unwrap();
    let misnamed_hash = Hash::of(&misnamed_bytes);
    bob.store
        .put(&bob_folder.link(1, &l1), misnamed_bytes)
        .await
        .unwrap();

    let report = w.dog.poll().await.unwrap();
    let mut flagged = report.inconsistent.clone();
    flagged.sort();
    let mut expected = vec![forged_hash, misnamed_hash];
    expected.sort();
    assert_eq!(flagged, expected, "{report:?}");
    let receipt_of = |h: &Hash| {
        w.dog
            .receipts()
            .find(|i| i.record == *h)
            .map(|i| i.receipt.payload.clone())
            .unwrap()
    };
    assert!(!receipt_of(&forged_hash).consistent);
    assert!(!receipt_of(&misnamed_hash).consistent);
    assert!(receipt_of(&l2).consistent, "Carol's proposal is fine");
    let bob_kid = bob.signer.kid();
    let bobs_real_vote = w
        .dog
        .receipts()
        .find(|i| {
            let p = &i.receipt.payload;
            p.by == bob_kid && p.seq == Some(2) && p.typ == typ::CONFIRM && p.consistent
        })
        .expect("Bob's first vote stands");
    assert_ne!(bobs_real_vote.record, forged_hash);
}

#[tokio::test]
async fn the_mirror_tier_keeps_byte_copies() {
    let mut w = world(Terms::mirror).await;
    let report = w.dog.poll().await.unwrap();
    assert_eq!(report.receipts, 3);
    assert_eq!(report.mirrored, 3, "genesis and both confirmations");
    let l1 = append(&mut w, 1, 4).await;
    let wf = Folder::from_path(&w.dog.signer().path()).witness(w.clients[0].chain());
    let copies = w
        .dog
        .store()
        .list(w.dog.store().me(), &format!("{}mirror/", wf.as_str()))
        .await
        .unwrap();
    let links: Vec<_> = copies
        .iter()
        .filter(|l| l.path.contains("/mirror/links/"))
        .collect();
    let confirms: Vec<_> = copies
        .iter()
        .filter(|l| l.path.contains("/mirror/confirms/"))
        .collect();
    assert_eq!(links.len(), 2, "{copies:?}");
    assert_eq!(confirms.len(), 4, "{copies:?}");
    let l1_copy = links
        .iter()
        .find(|l| l.path.ends_with(&format!("{}.jws", l1.h16())))
        .unwrap();
    let bytes = w
        .dog
        .store()
        .get(w.dog.store().me(), &l1_copy.path)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(Hash::of(&bytes), l1, "byte for byte");
    assert!(Signed::<Link>::decode(bytes, typ::LINK).is_ok());
}

#[tokio::test]
async fn a_lapsed_engagement_receipts_nothing() {
    let mut w = world(|_| Terms::receipts(NOW_S + 60)).await;
    assert_eq!(w.dog.poll().await.unwrap().receipts, 3);
    w.clock.advance(61_000);
    let l1 = w.clients[0]
        .propose("add", json!({ "n": 1 }))
        .await
        .unwrap();
    sync_all(&mut w.clients).await;
    w.clients[1].confirm(l1).await.unwrap();
    w.clients[2].confirm(l1).await.unwrap();
    let report = w.dog.poll().await.unwrap();
    assert!(report.lapsed);
    assert_eq!(report.receipts, 0);
    sync_all(&mut w.clients).await;
    let v = w.clients[0].verdict().unwrap();
    assert_eq!(
        v.committed_hashes().len(),
        2,
        "witnessing never gates the chain"
    );
    assert_eq!(v.committed[1].witnessed.0, 0);
}

// ── Operator: credit and markers (§11.2) ─────────────────────────────────────────────────────

/// A chain created by `actors[creator]` with the other listed actors as parties, naming
/// `witness` at genesis if given. Genesis is joined and committed; nothing is engaged.
async fn chain_among(
    actors: &[Actor],
    creator: usize,
    others: &[usize],
    witness: Option<String>,
    clock: &Clock,
) -> Vec<Client> {
    let mut order = vec![creator];
    order.extend_from_slice(others);
    let pubkies: Vec<String> = order.iter().map(|i| actors[*i].signer.pubky()).collect();
    let apps: Vec<&str> = order
        .iter()
        .map(|i| actors[*i].signer.client_id())
        .collect::<Vec<String>>()
        .leak()
        .iter()
        .map(String::as_str)
        .collect();
    let mut spec = GenesisSpec::new(pubkies.clone()).with_apps(&apps);
    spec.witnesses = witness.into_iter().collect();
    let first = ChainClient::create(
        Tally,
        actors[creator].store.clone(),
        actors[creator].signer.clone(),
        spec,
    )
    .await
    .unwrap()
    .with_clock(clock.reader());
    let chain = first.chain().clone();
    let initiator = (pubkies[0].clone(), actors[creator].signer.path());
    let mut clients = vec![first];
    for i in others {
        let mut c = ChainClient::open(
            Tally,
            actors[*i].store.clone(),
            actors[*i].signer.clone(),
            chain.clone(),
            initiator.clone(),
        )
        .with_clock(clock.reader());
        c.sync().await.unwrap();
        c.join().await.unwrap();
        clients.push(c);
    }
    sync_all(&mut clients).await;
    assert_eq!(committed(&clients[0]).len(), 1);
    clients
}

const DAY_S: u64 = 24 * 3600;

#[tokio::test]
async fn an_operator_engages_from_markers_and_renews_while_credit_lasts() {
    let shared = MemoryStore::shared();
    let clock = Clock::new(NOW_MS);
    let actors = vec![
        actor(&shared, "chess.example"),
        actor(&shared, "notes.example"),
        actor(&shared, "notes.example"),
        actor(&shared, "notes.example"),
    ];
    let (alice, bob, carol, dave) = (0, 1, 2, 3);
    let op_actor = actor(&shared, "watchman.example");
    let op_pubky = op_actor.signer.pubky();
    let mut op = Operator::new(
        op_actor.store.clone(),
        op_actor.signer.clone(),
        Terms::receipts(0).poll_every(1000),
    )
    .with_clock(clock.reader())
    .engage_for(2 * DAY_S)
    .renew_before(3600);
    op.free(actors[alice].signer.pubky());
    op.credit(actors[bob].signer.pubky(), 3 * DAY_S);

    // Nothing to do yet: no customer has a chain.
    assert_eq!(op.sweep().await.unwrap().engaged, vec![]);

    // Alice starts a chain naming the operator. Her client wrote `index/active/<id>` as a
    // matter of course; the operator finds it, reads genesis there, and engages for two days.
    let mut a = chain_among(
        &actors,
        alice,
        &[bob, carol],
        Some(op_pubky.clone()),
        &clock,
    )
    .await;
    let chain_a = a[0].chain().clone();
    let report = op.sweep().await.unwrap();
    assert_eq!(report.engaged, vec![chain_a.clone()]);
    assert!(report.declined.is_empty(), "{:?}", report.declined);
    assert_eq!(report.receipts, 3, "genesis and its QC, in the same sweep");
    let until_a = op.watching().next().unwrap().1.terms().until;
    assert_eq!(until_a, NOW_S + 2 * DAY_S);
    sync_all(&mut a).await;
    for c in &a {
        let v = c.verdict().unwrap();
        assert_eq!(v.engaged.len(), 1);
        assert_eq!(v.engaged[0].until, until_a);
        assert_eq!(v.committed[0].witnessed, (1, 1));
    }
    // Bob is also a customer and also wrote a marker for chain A; one engagement per chain.
    assert_eq!(op.sweep().await.unwrap().engaged, vec![]);
    assert_eq!(
        op.credit_of(&actors[bob].signer.pubky()),
        Some(Credit::Seconds(3 * DAY_S))
    );

    // Carol and Dave are nobody's customers: a chain between them naming the operator is
    // never looked at, however much it asks. A chain of Alice's that does not name the
    // operator is looked at and declined.
    let _c = chain_among(&actors, carol, &[dave], Some(op_pubky.clone()), &clock).await;
    let d = chain_among(&actors, alice, &[carol], None, &clock).await;
    let report = op.sweep().await.unwrap();
    assert_eq!(report.engaged, vec![]);
    assert_eq!(
        report.declined,
        vec![(
            actors[alice].signer.pubky(),
            d[0].chain().clone(),
            Declined::NotNamed
        )]
    );

    // Bob starts a chain: two of his three days are drawn.
    let b = chain_among(&actors, bob, &[carol], Some(op_pubky.clone()), &clock).await;
    let chain_b = b[0].chain().clone();
    assert_eq!(op.sweep().await.unwrap().engaged, vec![chain_b.clone()]);
    assert_eq!(
        op.credit_of(&actors[bob].signer.pubky()),
        Some(Credit::Seconds(DAY_S))
    );

    // Within an hour of `until`, both are renewed: Alice's for free by another two days,
    // Bob's by the one day he has left. Parties see the later `until` govern (§11.2).
    clock.advance((2 * DAY_S - 1800) * 1000);
    let report = op.sweep().await.unwrap();
    let mut extended = report.extended.clone();
    extended.sort();
    let mut both = vec![chain_a.clone(), chain_b.clone()];
    both.sort();
    assert_eq!(extended, both);
    assert_eq!(
        op.credit_of(&actors[bob].signer.pubky()),
        Some(Credit::Seconds(0))
    );
    let until = |op: &Operator<MemoryStore, LocalSigner>, chain: &pubky_mayfly::hash::ChainId| {
        op.watching()
            .find(|(c, _)| *c == chain)
            .unwrap()
            .1
            .terms()
            .until
    };
    assert_eq!(until(&op, &chain_a), NOW_S + 4 * DAY_S);
    assert_eq!(until(&op, &chain_b), NOW_S + 3 * DAY_S);
    sync_all(&mut a).await;
    for c in &a {
        let v = c.verdict().unwrap();
        assert_eq!(v.engaged[0].until, NOW_S + 4 * DAY_S);
        assert!(
            v.anomalies.is_empty(),
            "a later until is not equivocation: {:?}",
            v.anomalies
        );
    }

    // A day and a bit later Bob's credit is spent: his chain lapses and is dropped. Alice's
    // has most of a day left and is not touched.
    clock.advance((DAY_S + 1800 + 1) * 1000);
    let report = op.sweep().await.unwrap();
    assert_eq!(report.lapsed, vec![chain_b.clone()]);
    assert_eq!(report.extended, vec![]);
    assert_eq!(op.watching().count(), 1);
    // Topping him up does not revive B: its marker was acted on and the engagement lapsed
    // honestly. A new chain of his is engaged — and, with a free party in it, charged to
    // her, not to him.
    op.credit(actors[bob].signer.pubky(), DAY_S);
    assert_eq!(op.sweep().await.unwrap().engaged, vec![]);
    let b2 = chain_among(&actors, bob, &[alice], Some(op_pubky.clone()), &clock).await;
    assert_eq!(
        op.sweep().await.unwrap().engaged,
        vec![b2[0].chain().clone()]
    );
    assert_eq!(
        op.credit_of(&actors[bob].signer.pubky()),
        Some(Credit::Seconds(DAY_S))
    );
    // Alone with a non-customer, he pays.
    let b3 = chain_among(&actors, bob, &[carol], Some(op_pubky), &clock).await;
    assert_eq!(
        op.sweep().await.unwrap().engaged,
        vec![b3[0].chain().clone()]
    );
    assert_eq!(
        op.credit_of(&actors[bob].signer.pubky()),
        Some(Credit::Seconds(0))
    );
}

#[tokio::test]
async fn a_finished_marker_lets_the_engagement_lapse() {
    let shared = MemoryStore::shared();
    let clock = Clock::new(NOW_MS);
    let actors = vec![
        actor(&shared, "chess.example"),
        actor(&shared, "notes.example"),
    ];
    let op_actor = actor(&shared, "watchman.example");
    let mut op = Operator::new(
        op_actor.store.clone(),
        op_actor.signer.clone(),
        Terms::receipts(0),
    )
    .with_clock(clock.reader())
    .engage_for(DAY_S)
    .renew_before(3600);
    op.free(actors[0].signer.pubky());
    let mut clients = chain_among(&actors, 0, &[1], Some(op_actor.signer.pubky()), &clock).await;
    let chain = clients[0].chain().clone();
    assert_eq!(op.sweep().await.unwrap().engaged, vec![chain.clone()]);

    // The parties agree to stop; when the close commits, Alice's client moves her marker to
    // `index/finished/`, and the operator stops renewing.
    let close = clients[0].propose_close(CloseReason::Agreed).await.unwrap();
    sync_all(&mut clients).await;
    clients[1].confirm(close).await.unwrap();
    op.sweep().await.unwrap();
    sync_all(&mut clients).await;
    assert!(clients[0].verdict().unwrap().is_final());
    let folder = Folder::from_path(&actors[0].signer.path());
    let alice = actors[0].store.me().to_string();
    assert!(actors[0]
        .store
        .get(&alice, &folder.active(&chain))
        .await
        .unwrap()
        .is_none());
    assert!(actors[0]
        .store
        .get(&alice, &folder.finished(&chain))
        .await
        .unwrap()
        .is_some());

    clock.advance((DAY_S - 1800) * 1000);
    let report = op.sweep().await.unwrap();
    assert_eq!(report.extended, vec![], "a finished chain is not renewed");
    clock.advance(1801 * 1000);
    assert_eq!(op.sweep().await.unwrap().lapsed, vec![chain]);
    assert_eq!(op.watching().count(), 0);
    // The close itself was witnessed while the engagement ran.
    let v = clients[1].verdict().unwrap();
    assert_eq!(v.head().unwrap().witnessed, (1, 1));
}
