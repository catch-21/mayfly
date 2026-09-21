//! Phase 5 (§16.2) against a real homeserver: three grant sessions maintain a chain while a
//! fourth, on its own app, is the watchdog genesis names. The parties find its engagement from
//! its `/pub/` alone, every link comes out *witnessed 1/1*, receipts are embedded and mirrored,
//! and an anonymous verifier sees the same from any party's folder.
//!
//! Needs a Postgres, like `pubky-mayfly-client`'s `tests/testnet.rs`; `#[ignore]`d for that
//! reason. `tests/watchdog.rs` covers the same flow over the in-memory store.

use pubky_testnet::pubky::{ClientId, Keypair, Pubky};
use pubky_testnet::pubky_homeserver::ConfigToml;
use pubky_testnet::EphemeralTestnet;
use serde_json::json;

use pubky_mayfly::sim::Tally;
use pubky_mayfly_client::chain::verify_from;
use pubky_mayfly_client::{ChainClient, GenesisSpec, PubkyStore, SessionSigner, Signer, Store};
use pubky_mayfly_watchdog::{Terms, Watchdog};

type Client = ChainClient<Tally, PubkyStore, SessionSigner>;

async fn session(
    pubky: &Pubky,
    homeserver: &pubky_testnet::pubky::PublicKey,
    app: &str,
) -> (PubkyStore, SessionSigner) {
    let signer = pubky.signer(Keypair::random());
    signer.signup(homeserver, None).await.unwrap();
    let session = signer.signin(ClientId::new(app).unwrap()).await.unwrap();
    let store = PubkyStore::new(pubky.clone(), session.clone());
    let signer = SessionSigner::from_session(&session).await.unwrap();
    (store, signer)
}

async fn sync_all(clients: &mut [Client]) {
    for c in clients.iter_mut() {
        c.sync().await.unwrap();
        c.mirror().await.unwrap();
    }
}

fn now_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[tokio::test]
#[pubky_testnet::test]
#[ignore = "needs a Postgres for the homeserver; run with `cargo test -p pubky-mayfly-watchdog --test testnet -- --ignored`"]
async fn a_watchdog_session_receipts_a_chain_on_a_homeserver() {
    let testnet = EphemeralTestnet::builder()
        .config(ConfigToml::default_test_config())
        .build()
        .await
        .expect("a testnet needs a Postgres; see the module documentation");
    let homeserver = testnet.homeserver_app().public_key();
    let pubky = testnet.sdk().unwrap();

    let apps = ["chess.example", "notes.example", "notes.example"];
    let mut parties = Vec::new();
    for app in apps {
        parties.push(session(&pubky, &homeserver, app).await);
    }
    let (dog_store, dog_signer) = session(&pubky, &homeserver, "watchdog.example").await;
    let pubkies: Vec<String> = parties.iter().map(|(_, s)| s.pubky()).collect();

    let mut spec = GenesisSpec::new(pubkies.clone()).with_apps(&apps);
    spec.witnesses = vec![dog_signer.pubky()];
    let alice = ChainClient::create(Tally, parties[0].0.clone(), parties[0].1.clone(), spec)
        .await
        .unwrap();
    let chain = alice.chain().clone();
    let initiator = (pubkies[0].clone(), parties[0].1.path());

    let mut dog = Watchdog::new(
        dog_store,
        dog_signer.clone(),
        chain.clone(),
        initiator.clone(),
        Terms::receipts(now_s() + 3600),
    );
    dog.engage().await.unwrap();

    let mut clients = vec![alice];
    for (store, signer) in parties.iter().skip(1) {
        let mut c = ChainClient::open(
            Tally,
            store.clone(),
            signer.clone(),
            chain.clone(),
            initiator.clone(),
        );
        c.sync().await.unwrap();
        c.join().await.unwrap();
        clients.push(c);
    }
    sync_all(&mut clients).await;
    for c in &clients {
        let v = c.verdict().unwrap();
        assert_eq!(v.committed_hashes().len(), 1);
        assert_eq!(
            v.engaged.len(),
            1,
            "found from the watchdog's /pub/ listing"
        );
        assert_eq!(v.engaged[0].kid, dog_signer.kid());
        assert_eq!(v.committed[0].witnessed, (0, 1));
    }

    let report = dog.poll().await.unwrap();
    assert_eq!(report.receipts, 3);
    sync_all(&mut clients).await;
    for c in &clients {
        assert_eq!(c.verdict().unwrap().committed[0].witnessed, (1, 1));
    }

    let l1 = clients[0].propose("add", json!({ "n": 5 })).await.unwrap();
    sync_all(&mut clients).await;
    dog.poll().await.unwrap();
    clients[1].confirm(l1).await.unwrap();
    clients[2].confirm(l1).await.unwrap();
    dog.poll().await.unwrap();
    sync_all(&mut clients).await;
    let l2 = clients[1].propose("add", json!({ "n": 2 })).await.unwrap();
    sync_all(&mut clients).await;
    clients[0].confirm(l2).await.unwrap();
    clients[2].confirm(l2).await.unwrap();
    let report = dog.poll().await.unwrap();
    assert!(report.inconsistent.is_empty(), "{report:?}");
    sync_all(&mut clients).await;
    for c in &clients {
        let v = c.verdict().unwrap();
        assert_eq!(v.committed_hashes()[1..], [l1, l2]);
        for k in &v.committed {
            assert_eq!(k.witnessed, (1, 1), "seq {}", k.link.payload.seq);
        }
        assert_eq!(
            v.committed[2].link.payload.receipts.len(),
            1,
            "l2 embeds l1's receipt"
        );
        assert!(v.anomalies.is_empty(), "{:?}", v.anomalies);
        assert_eq!(c.state().unwrap().unwrap().total, 7);
    }

    // Causal order held across sweeps on real storage.
    for c in clients[0].verdict().unwrap().committed.windows(2) {
        let ready = c[0]
            .qc
            .iter()
            .map(|q| dog.observed_at(&q.hash).unwrap())
            .max()
            .unwrap_or(0);
        assert!(dog.observed_at(&c[1].link.hash).unwrap() >= ready);
    }

    // Anyone verifies the same from any party's folder, engagement and receipts included.
    let observer = PubkyStore::read_only(testnet.sdk().unwrap());
    for c in &clients {
        let report = verify_from(
            &Tally,
            &observer,
            &chain,
            (c.store().me().to_string(), c.signer().path()),
        )
        .await
        .unwrap();
        assert_eq!(
            report.verdict.committed_hashes(),
            clients[0].verdict().unwrap().committed_hashes()
        );
        assert_eq!(report.verdict.engaged.len(), 1);
        for k in &report.verdict.committed {
            assert_eq!(k.witnessed, (1, 1));
        }
        assert_eq!(report.folders.len(), 4, "three parties and the watchdog");
    }
}
