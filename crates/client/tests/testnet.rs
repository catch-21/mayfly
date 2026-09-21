//! Phase 2 against a real homeserver (§16.2): three grant sessions on an `EphemeralTestnet`,
//! under two `client_id`s, maintain a chain through `PubkyStore` and `SessionSigner`; a fourth,
//! anonymous `Pubky` verifies it from each party's homeserver folder; a recover moves a
//! party's folder and the verifier follows.
//!
//! The homeserver needs a Postgres: a local server on the default port, or
//! `TEST_PUBKY_CONNECTION_STRING`, or `pubky-testnet`'s `docker-postgres` feature. Without one
//! the testnet cannot start, so these tests are `#[ignore]`d and run with `-- --ignored`;
//! `tests/flows.rs` covers the same flows over the in-memory store on any machine.

use std::time::Duration;

use pubky_testnet::pubky::{ClientId, Keypair, Pubky};
use pubky_testnet::pubky_homeserver::ConfigToml;
use pubky_testnet::EphemeralTestnet;
use serde_json::json;

use pubky_mayfly::fold::Status;
use pubky_mayfly::hash::Hash;
use pubky_mayfly::sim::Tally;
use pubky_mayfly_client::chain::verify_from;
use pubky_mayfly_client::{ChainClient, GenesisSpec, PubkyStore, SessionSigner, Signer, Store};

type Client = ChainClient<Tally, PubkyStore, SessionSigner>;

async fn testnet() -> EphemeralTestnet {
    EphemeralTestnet::builder()
        .config(ConfigToml::default_test_config())
        .build()
        .await
        .expect("a testnet needs a Postgres; see the module documentation")
}

/// Sign an identity up on the homeserver and sign in through `app`.
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

fn committed(c: &Client) -> Vec<Hash> {
    c.verdict().unwrap().committed_hashes()
}

async fn sync_all(clients: &mut [Client]) {
    for c in clients.iter_mut() {
        c.sync().await.unwrap();
        c.mirror().await.unwrap();
    }
}

#[tokio::test]
#[pubky_testnet::test]
#[ignore = "needs a Postgres for the homeserver; run with `cargo test -p pubky-mayfly-client --test testnet -- --ignored`"]
async fn three_sessions_maintain_a_chain_and_anyone_verifies_it() {
    let testnet = testnet().await;
    let homeserver = testnet.homeserver_app().public_key();
    let pubky = testnet.sdk().unwrap();

    let apps = ["chess.example", "notes.example", "notes.example"];
    let mut parties = Vec::new();
    for app in apps {
        parties.push(session(&pubky, &homeserver, app).await);
    }
    let pubkies: Vec<String> = parties.iter().map(|(_, s)| s.pubky()).collect();

    // Alice writes genesis; Bob and Carol find it at her folder and join.
    let alice = ChainClient::create(
        Tally,
        parties[0].0.clone(),
        parties[0].1.clone(),
        GenesisSpec::new(pubkies.clone()).with_apps(&apps),
    )
    .await
    .unwrap();
    let chain = alice.chain().clone();
    let initiator = (pubkies[0].clone(), parties[0].1.path());
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
        assert_eq!(committed(c).len(), 1, "genesis committed");
    }

    // An append, confirmed by the others; Alice learns of the votes from the event stream.
    let l1 = clients[0].propose("add", json!({ "n": 5 })).await.unwrap();
    sync_all(&mut clients).await;
    clients[1].confirm(l1).await.unwrap();
    let (alice, rest) = clients.split_at_mut(1);
    let carol = &mut rest[1];
    let (seen, confirmed) = tokio::join!(alice[0].wait_for_event(Duration::from_secs(10)), async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        carol.confirm(l1).await
    });
    confirmed.unwrap();
    assert!(seen.unwrap(), "Alice saw a vote land on a homeserver");
    sync_all(&mut clients).await;
    for c in &clients {
        assert_eq!(committed(c).len(), 2);
        assert_eq!(committed(c)[1], l1);
        assert_eq!(c.state().unwrap().unwrap().total, 5);
        assert_eq!(c.verdict().unwrap().status, Status::Ongoing);
        assert!(
            c.verdict().unwrap().anomalies.is_empty(),
            "{:?}",
            c.verdict().unwrap().anomalies
        );
    }

    // Competing proposals converge through a dead round.
    let la = clients[1].propose("add", json!({ "n": 1 })).await.unwrap();
    clients[2].sync().await.unwrap();
    clients[2].confirm(la).await.unwrap();
    let _lb = clients[0].propose("add", json!({ "n": 2 })).await.unwrap();
    sync_all(&mut clients).await;
    assert!(clients[0].verdict().unwrap().open.as_ref().unwrap().dead);
    let lowest = clients[0].lowest_voted_in_dead_round().unwrap().unwrap();
    let mut reproposed = None;
    for c in clients.iter_mut() {
        if let Ok(h) = c.repropose(lowest).await {
            reproposed = Some(h);
        }
    }
    let l = reproposed.expect("one designated proposer");
    sync_all(&mut clients).await;
    for c in clients.iter_mut() {
        let _ = c.confirm(l).await; // the proposer is refused: already voted
    }
    sync_all(&mut clients).await;
    for c in &clients {
        assert_eq!(committed(c).len(), 3);
        assert_eq!(committed(c)[2], l);
    }

    // A fourth, anonymous Pubky verifies from each party's homeserver folder alone.
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
        assert_eq!(report.verdict.committed_hashes(), committed(&clients[0]));
        assert!(report.suspects.is_empty());
        assert_eq!(report.folders.len(), 3);
    }
}

#[tokio::test]
#[pubky_testnet::test]
#[ignore = "needs a Postgres for the homeserver; run with `cargo test -p pubky-mayfly-client --test testnet -- --ignored`"]
async fn recover_moves_a_folder_on_a_homeserver() {
    let testnet = testnet().await;
    let homeserver = testnet.homeserver_app().public_key();
    let pubky = testnet.sdk().unwrap();

    // Keep the identity signers so Carol can be signed in again through another app.
    let identities: Vec<_> = (0..3).map(|_| pubky.signer(Keypair::random())).collect();
    let apps = ["chess.example", "notes.example", "notes.example"];
    let mut parties = Vec::new();
    for (identity, app) in identities.iter().zip(apps) {
        identity.signup(&homeserver, None).await.unwrap();
        let session = identity.signin(ClientId::new(app).unwrap()).await.unwrap();
        let store = PubkyStore::new(pubky.clone(), session.clone());
        let signer = SessionSigner::from_session(&session).await.unwrap();
        parties.push((store, signer));
    }
    let pubkies: Vec<String> = parties.iter().map(|(_, s)| s.pubky()).collect();
    let alice = ChainClient::create(
        Tally,
        parties[0].0.clone(),
        parties[0].1.clone(),
        GenesisSpec::new(pubkies.clone()).with_apps(&apps),
    )
    .await
    .unwrap();
    let chain = alice.chain().clone();
    let initiator = (pubkies[0].clone(), parties[0].1.path());
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

    let old_path = clients[2].signer().path();
    let new_session = identities[2]
        .signin(ClientId::new("recovered.example").unwrap())
        .await
        .unwrap();
    let new_signer = SessionSigner::from_session(&new_session).await.unwrap();
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
            .find(|s| s.pubky == pubkies[2])
            .cloned()
            .unwrap();
        assert_eq!(seat.paths, vec![old_path.clone(), new_path.clone()]);
    }
    assert_eq!(clients[2].signer().path(), new_path);
    let l = clients[2].propose("add", json!({ "n": 7 })).await.unwrap();
    sync_all(&mut clients).await;
    clients[0].confirm(l).await.unwrap();
    clients[1].confirm(l).await.unwrap();
    sync_all(&mut clients).await;
    let observer = PubkyStore::read_only(testnet.sdk().unwrap());
    let report = verify_from(&Tally, &observer, &chain, initiator)
        .await
        .unwrap();
    assert_eq!(report.verdict.committed_hashes(), committed(&clients[0]));
    assert!(report.folders.contains(&(pubkies[2].clone(), new_path)));
}
