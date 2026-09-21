//! Phase 2 flows (§16.2) over the in-memory store: three parties on two apps maintain a chain;
//! a fourth, seatless verifier reads it from any one folder; competing proposals converge
//! through a dead round; an offline member stalls and resumes; a `recover` moves a party's
//! folder and the verifier follows; a tampered mirror is detected.
//!
//! `tests/testnet.rs` runs the same flows over `PubkyStore` against an `EphemeralTestnet`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use pubky_common::crypto::Keypair;
use serde_json::json;

use pubky_mayfly::fold::{AnomalyKind, Status};
use pubky_mayfly::hash::Hash;
use pubky_mayfly::sim::{Storage, Tally};
use pubky_mayfly_client::chain::verify_from;
use pubky_mayfly_client::{ChainClient, GenesisSpec, LocalSigner, MemoryStore, Signer, Store};

type Client = ChainClient<Tally, MemoryStore, LocalSigner>;

const NOW_S: u64 = 1_757_779_812;
const TWO_YEARS: u64 = 2 * 365 * 24 * 3600;

struct Party {
    identity: Keypair,
    signer: LocalSigner,
    store: MemoryStore,
}

fn party(shared: &Arc<Mutex<Storage>>, client_id: &str) -> Party {
    let identity = Keypair::random();
    let signer = LocalSigner::mint(&identity, client_id, NOW_S, TWO_YEARS);
    let store = MemoryStore::new(identity.public_key().z32(), Arc::clone(shared));
    Party {
        identity,
        signer,
        store,
    }
}

/// Alice on `chess.example`, Bob and Carol on `notes.example`; genesis created, joined and
/// committed. Returns the three clients, their identity keys (what Ring would hold) and the
/// shared storage.
async fn three_party_chain() -> (Vec<Client>, Vec<Keypair>, Arc<Mutex<Storage>>) {
    let shared = MemoryStore::shared();
    let parties = [
        party(&shared, "chess.example"),
        party(&shared, "notes.example"),
        party(&shared, "notes.example"),
    ];
    let pubkies: Vec<String> = parties.iter().map(|p| p.signer.pubky()).collect();
    let alice = ChainClient::create(
        Tally,
        parties[0].store.clone(),
        parties[0].signer.clone(),
        GenesisSpec::new(pubkies.clone()).with_apps(&[
            "chess.example",
            "notes.example",
            "notes.example",
        ]),
    )
    .await
    .unwrap();
    let chain = alice.chain().clone();
    let initiator = (pubkies[0].clone(), parties[0].signer.path());
    let mut clients = vec![alice];
    for p in &parties[1..] {
        let mut c = ChainClient::open(
            Tally,
            p.store.clone(),
            p.signer.clone(),
            chain.clone(),
            initiator.clone(),
        );
        c.sync().await.unwrap();
        assert!(
            c.verdict().unwrap().committed.is_empty(),
            "genesis is a proposal until confirmed"
        );
        c.join().await.unwrap();
        clients.push(c);
    }
    for c in &mut clients {
        let report = c.sync().await.unwrap();
        assert_eq!(
            report.verdict.committed_hashes().len(),
            1,
            "genesis committed"
        );
        assert_eq!(report.verdict.status, Status::Ongoing);
        c.mirror().await.unwrap();
    }
    let identities = parties.into_iter().map(|p| p.identity).collect();
    (clients, identities, shared)
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

#[tokio::test]
async fn three_parties_on_two_apps_append_and_agree() {
    let (mut clients, _, _shared) = three_party_chain().await;

    // Alice proposes; Bob and Carol confirm; everyone converges on the same chain.
    let l1 = clients[0].propose("add", json!({ "n": 5 })).await.unwrap();
    sync_all(&mut clients).await;
    clients[1].confirm(l1).await.unwrap();
    sync_all(&mut clients).await;
    assert_eq!(
        committed(&clients[2]).len(),
        1,
        "one confirmation is not a QC of three"
    );
    clients[2].confirm(l1).await.unwrap();
    sync_all(&mut clients).await;
    for c in &clients {
        assert_eq!(committed(c).len(), 2);
        assert_eq!(committed(c)[1], l1);
        assert_eq!(c.state().unwrap().unwrap().total, 5);
    }

    // Round 1 belongs to the designated proposer; anyone else is refused before writing.
    let l2 = clients[2].propose("add", json!({ "n": 1 })).await.unwrap();
    sync_all(&mut clients).await;
    assert!(matches!(
        clients[1].confirm(Hash::of(b"nothing")).await,
        Err(pubky_mayfly_client::Error::NoSuchCandidate)
    ));
    assert!(
        matches!(
            clients[2].confirm(l2).await,
            Err(pubky_mayfly_client::Error::AlreadyVoted { seq: 2, round: 0 })
        ),
        "a proposal is already its author's vote"
    );
    clients[0].confirm(l2).await.unwrap();
    clients[1].confirm(l2).await.unwrap();
    sync_all(&mut clients).await;
    for c in &clients {
        assert_eq!(committed(c).len(), 3);
        assert!(
            c.verdict().unwrap().anomalies.is_empty(),
            "{:?}",
            c.verdict().unwrap().anomalies
        );
    }

    // A seatless verifier reads the chain from any one party's folder.
    let observer = MemoryStore::new("", clients[0].store().storage());
    for c in &clients {
        let report = verify_from(
            &Tally,
            &observer,
            clients[0].chain(),
            (c.store().me().to_string(), c.signer().path()),
        )
        .await
        .unwrap();
        assert_eq!(report.verdict.committed_hashes(), committed(&clients[0]));
        assert!(report.suspects.is_empty());
        assert_eq!(
            report.folders.len(),
            3,
            "every declared folder was found from one"
        );
    }
}

#[tokio::test]
async fn competing_proposals_converge_through_a_dead_round() {
    let (mut clients, _, _shared) = three_party_chain().await;

    // Alice proposes; Carol sees it and confirms; Bob, who had not yet seen Alice's, proposes
    // his own. Two votes for different links: round 0 is dead under unanimity, and a party
    // who can see that will not vote in it.
    let la = clients[0].propose("add", json!({ "n": 1 })).await.unwrap();
    clients[2].sync().await.unwrap();
    clients[2].confirm(la).await.unwrap();
    let lb = clients[1].propose("add", json!({ "n": 2 })).await.unwrap();
    sync_all(&mut clients).await;
    let open = clients[0].verdict().unwrap().open.clone().unwrap();
    assert_eq!(open.seq, 1);
    assert!(open.dead, "two votes for different links kill round 0");
    assert!(matches!(
        clients[0].confirm(lb).await,
        Err(pubky_mayfly_client::Error::RoundDead)
    ));
    assert!(matches!(
        clients[1].confirm(la).await,
        Err(pubky_mayfly_client::Error::RoundDead)
    ));

    // Round 1: exactly one party may propose, and re-proposes the lowest-voted link.
    let lowest = clients[0].lowest_voted_in_dead_round().unwrap().unwrap();
    assert_eq!(lowest, la.min(lb));
    let mut reproposed = None;
    for c in clients.iter_mut() {
        match c.repropose(lowest).await {
            Ok(h) => reproposed = Some(h),
            Err(pubky_mayfly_client::Error::NotDesignated { round: 1, .. }) => {}
            Err(e) => panic!("{e}"),
        }
    }
    let l = reproposed.expect("one designated proposer");
    sync_all(&mut clients).await;
    for c in clients.iter_mut() {
        match c.confirm(l).await {
            Ok(_) => {}
            Err(pubky_mayfly_client::Error::AlreadyVoted { round: 1, .. }) => {} // the proposer
            Err(e) => panic!("{e}"),
        }
    }
    sync_all(&mut clients).await;
    for c in &clients {
        assert_eq!(committed(c).len(), 2);
        assert_eq!(committed(c)[1], l);
        assert_eq!(c.verdict().unwrap().head().unwrap().link.payload.round, 1);
        assert!(
            c.verdict().unwrap().anomalies.is_empty(),
            "{:?}",
            c.verdict().unwrap().anomalies
        );
    }
}

#[tokio::test]
async fn an_offline_member_stalls_the_chain_and_resumes_it() {
    let (mut clients, _, _shared) = three_party_chain().await;
    let l1 = clients[0].propose("add", json!({ "n": 3 })).await.unwrap();
    clients[1].sync().await.unwrap();
    clients[1].confirm(l1).await.unwrap();
    // Carol is offline: nothing more happens.
    clients[0].sync().await.unwrap();
    let v = clients[0].verdict().unwrap();
    assert!(matches!(
        v.status,
        Status::Stalled {
            seq: 1,
            round: 0,
            dead: false,
            ..
        }
    ));
    assert_eq!(committed(&clients[0]).len(), 1);
    assert!(!clients[0]
        .wait_for_change(Duration::from_millis(300))
        .await
        .unwrap());
    // Carol returns, syncs — sees the proposal and Bob's vote — and confirms, while Alice is
    // waiting for anything to change.
    clients[2].sync().await.unwrap();
    let (alice, rest) = clients.split_at_mut(1);
    let carol = &mut rest[1];
    let (changed, confirmed) =
        tokio::join!(alice[0].wait_for_change(Duration::from_secs(5)), async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            carol.confirm(l1).await
        });
    assert!(changed.unwrap(), "Alice saw Carol's vote land");
    confirmed.unwrap();
    sync_all(&mut clients).await;
    for c in &clients {
        assert_eq!(committed(c).len(), 2);
        assert_eq!(c.verdict().unwrap().status, Status::Ongoing);
    }
}

#[tokio::test]
async fn recover_moves_a_folder_and_the_verifier_follows() {
    let (mut clients, identities, shared) = three_party_chain().await;
    let l1 = clients[1].propose("add", json!({ "n": 1 })).await.unwrap();
    sync_all(&mut clients).await;
    clients[0].confirm(l1).await.unwrap();
    clients[2].confirm(l1).await.unwrap();
    sync_all(&mut clients).await;

    // Carol lost her key: the identity (in Ring) issues a Grant to a new app, with a new key,
    // and so a new folder.
    let old_path = clients[2].signer().path();
    let new_signer = LocalSigner::mint(&identities[2], "recovered.example", NOW_S, TWO_YEARS);
    let new_path = new_signer.path();
    let r = clients[2].propose_recover(new_signer).await.unwrap();
    sync_all(&mut clients).await;
    clients[0].confirm(r).await.unwrap();
    clients[1].confirm(r).await.unwrap();
    sync_all(&mut clients).await;
    for c in &clients {
        assert_eq!(committed(c).last(), Some(&r));
        let seat = c
            .verdict()
            .unwrap()
            .seats
            .iter()
            .find(|s| s.pubky == clients[2].signer().pubky())
            .unwrap()
            .clone();
        assert_eq!(seat.paths, vec![old_path.clone(), new_path.clone()]);
        assert_eq!(seat.client_id, "recovered.example");
    }
    // Carol's client adopted the new signer and writes to the new folder.
    assert_eq!(clients[2].signer().path(), new_path);
    let l3 = clients[2].propose("add", json!({ "n": 7 })).await.unwrap();
    sync_all(&mut clients).await;
    clients[0].confirm(l3).await.unwrap();
    clients[1].confirm(l3).await.unwrap();
    sync_all(&mut clients).await;
    for c in &clients {
        assert_eq!(committed(c).last(), Some(&l3));
        assert!(
            c.verdict().unwrap().anomalies.is_empty(),
            "{:?}",
            c.verdict().unwrap().anomalies
        );
    }
    // A verifier starting from Alice's folder follows the declared paths to Carol's new one.
    let observer = MemoryStore::new("", shared);
    let report = verify_from(
        &Tally,
        &observer,
        clients[0].chain(),
        (
            clients[0].store().me().to_string(),
            clients[0].signer().path(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(report.verdict.committed_hashes(), committed(&clients[0]));
    assert!(report
        .folders
        .contains(&(clients[2].signer().pubky(), new_path)));
}

#[tokio::test]
async fn a_tampered_mirror_is_detected_and_ignored() {
    let (mut clients, _, shared) = three_party_chain().await;
    let l1 = clients[0].propose("add", json!({ "n": 4 })).await.unwrap();
    sync_all(&mut clients).await;
    clients[1].confirm(l1).await.unwrap();
    clients[2].confirm(l1).await.unwrap();
    sync_all(&mut clients).await;
    let before = committed(&clients[0]);

    // Bob edits his mirror of Alice's link in place: same file name, different bytes.
    {
        let mut storage = shared.lock().unwrap();
        let bob = format!(
            "pubky://{}{}",
            clients[1].store().me(),
            clients[1].signer().path()
        );
        let files = storage.folders.get_mut(&bob).unwrap();
        let name = files
            .keys()
            .find(|k| k.contains("links/00000001-"))
            .cloned()
            .unwrap();
        let mut bytes = files[&name].clone();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0x01;
        files.insert(name, bytes);
    }
    let bob = clients[1].store().me().to_string();
    for c in clients.iter_mut() {
        let report = c.sync().await.unwrap();
        assert_eq!(
            report.verdict.committed_hashes(),
            before,
            "the original copies stand"
        );
        assert_eq!(report.suspects.len(), 1, "{:?}", report.suspects);
        assert_eq!(
            report.suspects[0].owner, bob,
            "attributed to the folder owner"
        );
    }
    let observer = MemoryStore::new("", shared);
    let report = verify_from(
        &Tally,
        &observer,
        clients[0].chain(),
        (
            clients[1].store().me().to_string(),
            clients[1].signer().path(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(report.verdict.committed_hashes(), before);
    assert_eq!(report.suspects.len(), 1);
    assert!(report
        .verdict
        .anomalies
        .iter()
        .all(|a| a.kind != AnomalyKind::HostileSource));
}
