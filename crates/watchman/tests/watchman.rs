//! Phase 5 (§16.2) over the in-memory store: a watchman engages, receipts a three-party chain
//! in causal order, and the parties' folds report every link *witnessed 1/1*, embed its
//! receipts and mirror them; an abandoned close is merely asserted until the watchman's
//! receipts adjudicate it; a party's reject is receipted as itself, and a second vote in that
//! round is receipted as inconsistent, as are a double vote and a misnamed mirror;
//! the `mirror` tier keeps byte copies; a lapsed engagement receipts nothing.
//!
//! `tests/testnet.rs` runs the first flow over `PubkyStore` against an `EphemeralTestnet`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pubky_common::crypto::Keypair;
use serde_json::json;

use pubky_mayfly::close::CloseState;
use pubky_mayfly::fold::{AnomalyKind, Status};
use pubky_mayfly::hash::{ChainId, Hash};
use pubky_mayfly::record::{CloseReason, Confirmation, Link, Signed};
use pubky_mayfly::sim::{Storage, Tally};
use pubky_mayfly::{typ, PROTOCOL_VERSION};
use pubky_mayfly_client::chain::verify_from;
use pubky_mayfly_client::layout::Folder;
use pubky_mayfly_client::{
    ChainClient, Error as ClientError, GenesisSpec, Listed, LocalSigner, MemoryStore, Signer, Store,
};
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
    /// The identity: a fresh sign-in mints a new client key for it (`Actor::sign_in_again`).
    identity: Keypair,
}

fn actor(shared: &Arc<Mutex<Storage>>, client_id: &str) -> Actor {
    let identity = Keypair::random();
    let signer = LocalSigner::mint(&identity, client_id, NOW_S, TWO_YEARS);
    let store = MemoryStore::new(identity.public_key().z32(), Arc::clone(shared));
    Actor {
        signer,
        store,
        identity,
    }
}

impl Actor {
    /// The same pubky signed in afresh: a new Grant, a new client key (`kid`), as a restarted
    /// service gets.
    fn sign_in_again(&self, now_s: u64) -> LocalSigner {
        LocalSigner::mint(&self.identity, &self.signer.client_id(), now_s, TWO_YEARS)
    }
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
    /// The watchman's identity and store, for a second process over the same folder.
    watchman: Actor,
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
        watchman,
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

    // The forged confirmation is a new file: a plain poll fetches it by name and finds it out.
    // The misnamed mirror overwrites a file the watchman has already read, so a plain poll —
    // names only — does not re-read it; the audit, which lists with content hashes, does.
    let report = w.dog.poll().await.unwrap();
    assert_eq!(report.inconsistent, vec![forged_hash], "{report:?}");
    let audit = w.dog.audit().await.unwrap();
    assert_eq!(audit.inconsistent, vec![misnamed_hash], "{audit:?}");
    let mut flagged = [report.inconsistent.clone(), audit.inconsistent.clone()].concat();
    flagged.sort();
    let mut expected = vec![forged_hash, misnamed_hash];
    expected.sort();
    assert_eq!(flagged, expected, "{report:?} {audit:?}");
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

/// A refusal is the party's vote, receipted like any other record. The watchman does not
/// become a voter by receipting it, and a later confirmation by the same key in that round
/// is a second vote: `consistent: false`, attributed to the party.
#[tokio::test]
async fn a_watchman_receipts_a_reject_and_flags_a_second_vote() {
    let mut w = world(Terms::receipts).await;
    w.dog.poll().await.unwrap();
    let l1 = w.clients[0]
        .propose("add", json!({ "n": 1 }))
        .await
        .unwrap();
    sync_all(&mut w.clients).await;
    w.clients[1].reject(l1).await.unwrap();
    sync_all(&mut w.clients).await;
    let report = w.dog.poll().await.unwrap();
    assert!(
        report.inconsistent.is_empty(),
        "a refusal is one vote, not a split view: {report:?}"
    );
    let bob_kid = w.actors[1].signer.kid();
    let dog_kid = w.dog.signer().kid();
    let reject_receipt = w
        .dog
        .receipts()
        .find(|i| {
            let p = &i.receipt.payload;
            p.by == bob_kid && p.typ == typ::REJECT && p.consistent
        })
        .expect("the refusal was receipted");
    assert_eq!(reject_receipt.receipt.payload.seq, Some(1));
    for c in &w.clients {
        let v = c.verdict().unwrap();
        assert!(
            v.anomalies.iter().any(|a| {
                a.kind == AnomalyKind::Obstruction && a.against.as_deref() == Some(bob_kid.as_str())
            }),
            "refusing the sole proposal is the party's obstruction: {:?}",
            v.anomalies
        );
        assert!(
            v.anomalies
                .iter()
                .all(|a| a.against.as_deref() != Some(dog_kid.as_str())),
            "the watchman did not vote: {:?}",
            v.anomalies
        );
        assert!(!v.is_final());
        assert_eq!(v.committed_hashes().len(), 1);
    }

    let bob = &w.actors[1];
    let chain = w.clients[0].chain().clone();
    let bob_folder = Folder::from_path(&bob.signer.path()).chain(&chain);
    let forged = Confirmation {
        v: PROTOCOL_VERSION,
        chain: chain.clone(),
        seq: 1,
        round: Some(0),
        link: l1.to_base64url(),
        kid: bob_kid.clone(),
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
        .put(&bob_folder.confirm(1, &l1), forged_bytes)
        .await
        .unwrap();
    let report = w.dog.poll().await.unwrap();
    assert!(
        report.inconsistent.contains(&forged_hash),
        "a confirm after a reject is a second vote: {report:?}"
    );
    let second = w
        .dog
        .receipts()
        .find(|i| i.record == forged_hash)
        .map(|i| i.receipt.payload.clone())
        .unwrap();
    assert!(!second.consistent);
    assert_eq!(second.by, bob_kid);
    sync_all(&mut w.clients).await;
    for c in &w.clients {
        let v = c.verdict().unwrap();
        assert!(
            v.anomalies.iter().any(|a| {
                a.kind == AnomalyKind::Equivocation
                    && a.against.as_deref() == Some(bob_kid.as_str())
            }),
            "{:?}",
            v.anomalies
        );
        assert!(
            v.anomalies
                .iter()
                .all(|a| a.against.as_deref() != Some(dog_kid.as_str())),
            "{:?}",
            v.anomalies
        );
    }
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

/// A mirror the watchman has already read is overwritten in place. A plain poll lists by name
/// and does not re-read it; an audit (content hashes) or an event naming the path does, and
/// the rewrite is receipted as inconsistent (§7, §11.3).
#[tokio::test]
async fn an_overwritten_mirror_is_caught_by_an_audit_or_an_event() {
    let mut w = world(Terms::receipts).await;
    w.dog.poll().await.unwrap();
    let l1 = append(&mut w, 0, 1).await;
    let l2 = append(&mut w, 2, 2).await;
    let chain = w.clients[0].chain().clone();
    let bob = &w.actors[1];
    let bob_folder = Folder::from_path(&bob.signer.path()).chain(&chain);
    // Bob's mirror of Alice's l1 exists and has been receipted as a copy.
    assert!(bob
        .store
        .get(bob.store.me(), &bob_folder.link(1, &l1))
        .await
        .unwrap()
        .is_some());
    let before = w.dog.receipts().count();

    let rewrite = |n: u64| {
        let head = w.clients[1].verdict().unwrap().head().unwrap().clone();
        let link = Link {
            v: PROTOCOL_VERSION,
            chain: chain.clone(),
            seq: 9,
            round: 0,
            prev: head.link.hash.to_base64url(),
            confirms: Vec::new(),
            receipts: Vec::new(),
            author: bob.signer.pubky(),
            kid: bob.signer.kid(),
            ts: w.clock.now(),
            kind: "add".into(),
            body: json!({ "n": n }),
            state: head.link.payload.state.clone(),
            grant: None,
        };
        pubky_mayfly::record::sign(bob.signer.keypair(), typ::LINK, &link).unwrap()
    };

    // First rewrite: a plain poll does not see it; an audit does.
    let first = rewrite(91);
    let first_hash = Hash::of(&first);
    bob.store
        .put(&bob_folder.link(1, &l1), first)
        .await
        .unwrap();
    let report = w.dog.poll().await.unwrap();
    assert_eq!(report.receipts, 0, "by name, a seen file is not re-read");
    let report = w.dog.audit().await.unwrap();
    assert_eq!(report.inconsistent, vec![first_hash], "{report:?}");
    assert_eq!(w.dog.receipts().count(), before + 1);

    // Second rewrite: an event on the path is enough for the next plain poll.
    let second = rewrite(92);
    let second_hash = Hash::of(&second);
    bob.store
        .put(&bob_folder.link(2, &l2), second)
        .await
        .unwrap();
    assert_eq!(w.dog.poll().await.unwrap().receipts, 0);
    w.dog.note_changed(bob.store.me(), &bob_folder.link(2, &l2));
    let report = w.dog.poll().await.unwrap();
    assert_eq!(report.inconsistent, vec![second_hash], "{report:?}");
    assert_eq!(w.dog.receipts().count(), before + 2);
}

/// A watchman that starts again over the same folder takes back its engagement and every
/// receipt it wrote, so no record is receipted twice and the true `observed_at` stands.
#[tokio::test]
async fn a_restart_continues_where_the_last_process_stopped() {
    let mut w = world(Terms::receipts).await;
    w.dog.poll().await.unwrap();
    append(&mut w, 0, 1).await;
    append(&mut w, 1, 2).await;
    let chain = w.clients[0].chain().clone();
    let me = w.dog.store().me().to_string();
    let witness = Folder::from_path(&w.dog.signer().path()).witness(&chain);
    let files_before = w
        .dog
        .store()
        .list(&me, witness.as_str())
        .await
        .unwrap()
        .len();
    let issued: BTreeMap<Hash, u64> = w
        .dog
        .receipts()
        .map(|i| (i.record, i.receipt.payload.observed_at))
        .collect();
    assert!(issued.len() >= 7, "genesis, two links, their votes");
    let until = w.dog.terms().until;

    // A new process, an hour later, over the same store and signer.
    w.clock.advance(HOUR_MS);
    let initiator = (w.actors[0].signer.pubky(), w.actors[0].signer.path());
    let mut again = Watchman::new(
        w.dog.store().clone(),
        w.dog.signer().clone(),
        chain.clone(),
        initiator,
        Terms::receipts(0),
    )
    .with_clock(w.clock.reader());
    assert!(again.discover().await.unwrap());
    let resumed = again.resume().await.unwrap();
    assert_eq!(
        resumed.until,
        Some(until),
        "the engagement on file, not shortened"
    );
    assert_eq!(resumed.receipts, issued.len());
    assert!(again.is_engaged());
    for (record, at) in &issued {
        assert_eq!(again.observed_at(record), Some(*at), "the true time stands");
    }

    // Its first poll reads every file once and writes nothing new.
    let report = again.poll().await.unwrap();
    assert_eq!(report.receipts, 0, "{report:?}");
    assert_eq!(
        again
            .store()
            .list(&me, witness.as_str())
            .await
            .unwrap()
            .len(),
        files_before,
        "no second receipt on file"
    );

    // New records after the restart are receipted as before, continuing the cursor.
    let max_cursor = again
        .receipts()
        .map(|i| i.receipt.payload.source.cursor)
        .max()
        .unwrap();
    w.clock.advance(60_000);
    let l3 = w.clients[2]
        .propose("add", json!({ "n": 3 }))
        .await
        .unwrap();
    sync_all(&mut w.clients).await;
    let report = again.poll().await.unwrap();
    assert_eq!(report.receipts, 1);
    let fresh = again.receipts().find(|i| i.record == l3).unwrap();
    assert!(fresh.receipt.payload.source.cursor > max_cursor);

    // The operator does the same from a marker: it resumes rather than engaging afresh, and
    // draws no credit for time the customer already paid for.
    let mut op = Operator::new(
        w.dog.store().clone(),
        w.dog.signer().clone(),
        Terms::receipts(0),
    )
    .with_clock(w.clock.reader())
    .engage_for(DAY_S);
    op.credit(w.actors[0].signer.pubky(), DAY_S);
    let report = op.sweep().await.unwrap();
    assert_eq!(report.resumed, vec![chain.clone()]);
    assert_eq!(report.engaged, vec![]);
    assert_eq!(report.receipts, 0);
    assert_eq!(
        op.credit_of(&w.actors[0].signer.pubky()),
        Some(Credit::Seconds(DAY_S))
    );
    assert_eq!(op.watching().next().unwrap().1.terms().until, until);

    // The service's own case: the same pubky signs in again and gets a new client key. The
    // receipts by the old key are its own and are taken back; the engagement on file was the
    // old key's, so a new one is published under the new key — ending no earlier than the
    // old, charged only for the time beyond it — and no record is receipted twice.
    let files_now = w
        .watchman
        .store
        .list(&me, witness.as_str())
        .await
        .unwrap()
        .len();
    let rekeyed = w.watchman.sign_in_again(w.clock.now() / 1000);
    assert_ne!(rekeyed.kid(), w.dog.signer().kid());
    assert_eq!(rekeyed.pubky(), w.dog.signer().pubky());
    let mut op2 = Operator::new(
        w.watchman.store.clone(),
        rekeyed.clone(),
        Terms::receipts(0),
    )
    .with_clock(w.clock.reader())
    .engage_for(DAY_S);
    op2.credit(w.actors[0].signer.pubky(), 10 * DAY_S);
    let report = op2.sweep().await.unwrap();
    assert_eq!(report.resumed, vec![chain.clone()], "{report:?}");
    assert_eq!(report.receipts, 0, "nothing receipted twice: {report:?}");
    let dog2 = op2.watching().next().unwrap().1;
    assert!(dog2.is_engaged());
    assert!(dog2.terms().until >= until, "never shortened");
    let now_s = w.clock.now() / 1000;
    assert_eq!(dog2.terms().until, (now_s + DAY_S).max(until));
    let charged = 10 * DAY_S - (dog2.terms().until - until.max(now_s));
    assert_eq!(
        op2.credit_of(&w.actors[0].signer.pubky()),
        Some(Credit::Seconds(charged)),
        "charged only for time beyond the engagement on file"
    );
    for (record, at) in &issued {
        assert_eq!(dog2.observed_at(record), Some(*at));
    }
    assert!(dog2.observed_at(&l3).is_some());
    let files_after = w.watchman.store.list(&me, witness.as_str()).await.unwrap();
    // One new `engage.jws` (overwritten in place) and one new historic `engage/<kid>.jws`;
    // no new receipt.
    assert_eq!(files_after.len(), files_now + 1, "{files_after:#?}");
    assert!(files_after
        .iter()
        .any(|f| f.path.ends_with(&format!("engage/{}.jws", rekeyed.kid()))));
    let receipts_on_file = files_after
        .iter()
        .filter(|f| !f.path.contains("/engage") && f.path.ends_with(".jws"))
        .count();
    assert_eq!(receipts_on_file, dog2.receipts().count());

    // And the parties' folds still see every link witnessed, under either key.
    sync_all(&mut w.clients).await;
    for l in &w.clients[0].verdict().unwrap().committed {
        assert_eq!(l.witnessed, (1, 1), "seq {}", l.link.payload.seq);
    }
}

/// A store whose reads of one owner never answer: a homeserver that has gone away.
#[derive(Clone)]
struct Stalling {
    inner: MemoryStore,
    stalled: Arc<Mutex<Option<String>>>,
}

impl Stalling {
    fn stall(&self, owner: Option<String>) {
        *self.stalled.lock().unwrap() = owner;
    }
    fn is_stalled(&self, owner: &str) -> bool {
        self.stalled.lock().unwrap().as_deref() == Some(owner)
    }
}

#[async_trait::async_trait]
impl Store for Stalling {
    fn me(&self) -> &str {
        self.inner.me()
    }
    async fn list(&self, owner: &str, prefix: &str) -> Result<Vec<Listed>, ClientError> {
        if self.is_stalled(owner) {
            std::future::pending::<()>().await;
        }
        self.inner.list(owner, prefix).await
    }
    async fn list_names(&self, owner: &str, prefix: &str) -> Result<Vec<Listed>, ClientError> {
        if self.is_stalled(owner) {
            std::future::pending::<()>().await;
        }
        self.inner.list_names(owner, prefix).await
    }
    async fn get(&self, owner: &str, path: &str) -> Result<Option<Vec<u8>>, ClientError> {
        if self.is_stalled(owner) {
            std::future::pending::<()>().await;
        }
        self.inner.get(owner, path).await
    }
    async fn put(&self, path: &str, bytes: Vec<u8>) -> Result<(), ClientError> {
        self.inner.put(path, bytes).await
    }
    async fn delete(&self, path: &str) -> Result<(), ClientError> {
        self.inner.delete(path).await
    }
}

/// One member's homeserver stops answering. The chain it is on runs out of time each sweep
/// and is reported; the other chain is receipted within the deadline as if nothing were
/// wrong; and when the homeserver returns, nothing on the stalled chain has been skipped.
#[tokio::test]
async fn a_slow_homeserver_holds_only_its_own_chain() {
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
    let store = Stalling {
        inner: op_actor.store.clone(),
        stalled: Arc::new(Mutex::new(None)),
    };
    let mut op = Operator::new(store.clone(), op_actor.signer.clone(), Terms::receipts(0))
        .with_clock(clock.reader())
        .engage_for(DAY_S)
        .deadline(Duration::from_millis(300))
        .concurrency(4);
    op.free(actors[alice].signer.pubky());
    op.free(actors[carol].signer.pubky());

    // Two chains: Alice with Bob, Carol with Dave.
    let mut ab = chain_among(
        &actors,
        alice,
        &[bob],
        Some(op_actor.signer.pubky()),
        &clock,
    )
    .await;
    let mut cd = chain_among(
        &actors,
        carol,
        &[dave],
        Some(op_actor.signer.pubky()),
        &clock,
    )
    .await;
    let (chain_ab, chain_cd) = (ab[0].chain().clone(), cd[0].chain().clone());
    let report = op.sweep().await.unwrap();
    let mut engaged = report.engaged.clone();
    engaged.sort();
    let mut both = vec![chain_ab.clone(), chain_cd.clone()];
    both.sort();
    assert_eq!(engaged, both);
    assert!(
        report.timed_out.is_empty() && report.failed.is_empty(),
        "{report:?}"
    );

    // Bob's homeserver goes away. Both chains move on.
    store.stall(Some(actors[bob].signer.pubky()));
    clock.advance(60_000);
    let l_ab = ab[0].propose("add", json!({ "n": 1 })).await.unwrap();
    let l_cd = cd[0].propose("add", json!({ "n": 1 })).await.unwrap();
    sync_all(&mut ab).await;
    sync_all(&mut cd).await;
    let started = std::time::Instant::now();
    let report = op.sweep().await.unwrap();
    let took = started.elapsed();
    assert_eq!(report.timed_out, vec![chain_ab.clone()], "{report:?}");
    assert!(report.failed.is_empty(), "{report:?}");
    assert!(
        took < Duration::from_millis(1500),
        "the sweep waited for one deadline, not for Bob: {took:?}"
    );
    let receipted = |op: &Operator<Stalling, LocalSigner>, chain: &ChainId, h: &Hash| {
        op.watching()
            .find(|(c, _)| *c == chain)
            .map(|(_, dog)| dog.observed_at(h).is_some())
            .unwrap_or(false)
    };
    assert!(receipted(&op, &chain_cd, &l_cd), "Carol's chain was served");
    assert!(
        !receipted(&op, &chain_ab, &l_ab),
        "Alice's is waiting on Bob"
    );
    // Alice's own folder holds the same link, but the poll that would have read it was cut
    // short; nothing was marked seen, so nothing is lost.

    // Bob's homeserver is back: the next sweep receipts what it could not.
    store.stall(None);
    let report = op.sweep().await.unwrap();
    assert!(report.timed_out.is_empty(), "{report:?}");
    assert!(receipted(&op, &chain_ab, &l_ab), "caught up: {report:?}");
    assert!(report.receipts >= 1);
    // Every receipt that exists is on file exactly once.
    let me = store.me().to_string();
    let witness = Folder::from_path(&op_actor.signer.path()).witness(&chain_ab);
    let files = store.inner.list(&me, witness.as_str()).await.unwrap();
    let receipts: Vec<_> = files
        .iter()
        .filter(|f| !f.path.contains("/engage") && f.path.ends_with(".jws"))
        .collect();
    let dog = op.watching().find(|(c, _)| **c == chain_ab).unwrap().1;
    assert_eq!(receipts.len(), dog.receipts().count());

    // A customer whose homeserver stalls during discovery is reported and skipped, not waited
    // for; the chain her folder is part of runs out of time like any other, and Alice's
    // chain, which never reads Carol, is untouched.
    store.stall(Some(actors[carol].signer.pubky()));
    let started = std::time::Instant::now();
    let report = op.sweep().await.unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(report.slow_customers, vec![actors[carol].signer.pubky()]);
    assert_eq!(report.timed_out, vec![chain_cd.clone()], "{report:?}");
    assert!(report.failed.is_empty(), "{report:?}");
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
