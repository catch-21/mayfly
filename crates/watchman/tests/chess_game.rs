//! Two players and a watchman play a game of chess.
//!
//! White and Black each drive a `ChainClient`. The watchman receipts every record. A move
//! commits only after the opponent's client has run the rules. Think time is the watchman's
//! gap from the previous quorum to the proposal; the opponent's wait is respond, and is not
//! added to think. A stranger replaying the files sees the same chain and the same times.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pubky_common::crypto::Keypair;
use serde_json::json;

use pubky_mayfly::close::CloseState;
use pubky_mayfly::fold::Status;
use pubky_mayfly::record::CloseReason;
use pubky_mayfly::sim::Storage;
use pubky_mayfly::witness::stopwatch;
use pubky_mayfly_client::chain::verify_from;
use pubky_mayfly_client::view::chain_view;
use pubky_mayfly_client::{
    Action, ChainClient, GenesisSpec, LocalSigner, MemoryStore, Signer, Store,
};
use pubky_mayfly_rules::chess::{chess_view, Body, Chess};
use pubky_mayfly_watchman::{Terms, Watchman};

type Client = ChainClient<Chess, MemoryStore, LocalSigner>;
type Dog = Watchman<MemoryStore, LocalSigner>;

const NOW_S: u64 = 1_757_779_812;
const NOW_MS: u64 = NOW_S * 1000;
const TWO_YEARS: u64 = 2 * 365 * 24 * 3600;
const THINK: u64 = 30_000;
const RESPOND: u64 = 10_000;

struct Table {
    clients: Vec<Client>,
    dog: Dog,
    clock: Clock,
    shared: Arc<Mutex<Storage>>,
}

#[derive(Clone)]
struct Clock(Arc<AtomicU64>);

impl Clock {
    fn new(ms: u64) -> Self {
        Self(Arc::new(AtomicU64::new(ms)))
    }
    fn advance(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
    fn reader(&self) -> impl Fn() -> u64 + Send + Sync + 'static {
        let c = self.clone();
        move || c.0.load(Ordering::SeqCst)
    }
}

fn signer(shared: &Arc<Mutex<Storage>>, app: &str) -> (LocalSigner, MemoryStore) {
    let identity = Keypair::random();
    let signer = LocalSigner::mint(&identity, app, NOW_S, TWO_YEARS);
    let store = MemoryStore::new(identity.public_key().z32(), Arc::clone(shared));
    (signer, store)
}

/// White, Black, and a watchman. Genesis is committed and receipted; nothing has been played.
async fn table(think_ms: u64) -> Table {
    let shared = MemoryStore::shared();
    let clock = Clock::new(NOW_MS);
    let white = signer(&shared, "chess.example");
    let black = signer(&shared, "chess.example");
    let watchman = signer(&shared, "watchman.example");
    let mut spec = GenesisSpec::new(vec![white.0.pubky(), black.0.pubky()])
        .with_apps(&["chess.example", "chess.example"]);
    spec.roles = vec![Some("white".into()), Some("black".into())];
    spec.witnesses = vec![watchman.0.pubky()];
    spec.options = json!({ "time_control": { "think_ms": think_ms, "respond_ms": think_ms } });
    let alice = ChainClient::create(Chess, white.1.clone(), white.0.clone(), spec)
        .await
        .unwrap()
        .with_clock(clock.reader());
    let chain = alice.chain().clone();
    let initiator = (white.0.pubky(), white.0.path());
    let mut dog = Watchman::new(
        watchman.1,
        watchman.0,
        chain.clone(),
        initiator.clone(),
        Terms::receipts(NOW_S + TWO_YEARS),
    )
    .with_clock(clock.reader());
    dog.engage().await.unwrap();

    let mut bob =
        ChainClient::open(Chess, black.1, black.0, chain, initiator).with_clock(clock.reader());
    bob.sync().await.unwrap();
    bob.join().await.unwrap();
    let mut clients = vec![alice, bob];
    for c in &mut clients {
        c.act().await.unwrap();
    }
    dog.poll().await.unwrap();
    for c in &mut clients {
        c.act().await.unwrap();
    }
    for c in &clients {
        let v = c.verdict().unwrap();
        assert_eq!(v.committed.len(), 1, "genesis committed");
        assert_eq!(v.engaged.len(), 1);
        assert_eq!(v.committed[0].witnessed, (1, 1));
        assert!(v.committed[0].observed_at.ms.is_some(), "genesis is timed");
        assert!(
            v.committed[0].think.ms.is_none(),
            "genesis has no previous quorum"
        );
        let state = c.state().unwrap().unwrap();
        assert_eq!(state.white, 0);
        assert_eq!(state.black, 1);
        assert!(state.result.is_none());
        assert_eq!(chess_view(&state).unwrap().legal_uci.len(), 20);
    }
    assert_eq!(
        clients[0].verdict().unwrap().committed_hashes(),
        clients[1].verdict().unwrap().committed_hashes()
    );
    Table {
        clients,
        dog,
        clock,
        shared,
    }
}

/// Propose `body`, let the watchman time it, then let the opponent confirm and time that too.
async fn propose(table: &mut Table, by: usize, body: Body, think: u64, respond: u64) {
    let before = table.clients[0].verdict().unwrap().committed.len();
    table.clock.advance(think);
    table.clients[by].propose_body(&body).await.unwrap();
    table.dog.poll().await.unwrap();
    table.clock.advance(respond);
    let acts = table.clients[1 - by].act().await.unwrap();
    assert!(
        acts.iter().any(|a| matches!(a, Action::Confirmed(_))),
        "the opponent confirms only a rules-legal proposal: {acts:?}"
    );
    table.dog.poll().await.unwrap();
    for c in &mut table.clients {
        c.act().await.unwrap();
    }
    let link = table.clients[0]
        .verdict()
        .unwrap()
        .committed
        .last()
        .unwrap()
        .clone();
    assert_eq!(
        table.clients[0].verdict().unwrap().committed.len(),
        before + 1
    );
    let prev = &table.clients[0].verdict().unwrap().committed[before - 1];
    let ready = prev
        .qc
        .iter()
        .map(|q| table.dog.observed_at(&q.hash).unwrap())
        .max()
        .unwrap();
    let proposal = table.dog.observed_at(&link.link.hash).unwrap();
    let confirmed = link
        .qc
        .iter()
        .map(|q| table.dog.observed_at(&q.hash).unwrap())
        .max()
        .unwrap();
    assert_eq!(link.observed_at.ms, Some(proposal));
    assert_eq!(link.confirmed_at.ms, Some(confirmed));
    assert_eq!(link.think.ms, Some(stopwatch::think(proposal, ready)));
    assert_eq!(link.think.ms, Some(think), "think is the proposer's wait");
    assert_eq!(
        link.respond.ms,
        Some(stopwatch::respond(confirmed, proposal))
    );
    assert_eq!(link.respond.ms, Some(respond), "confirming is not thinking");
    assert!(link.think.split.is_empty() && link.respond.split.is_empty());
    assert_eq!(link.witnessed, (1, 1));
    assert_eq!(
        table.clients[1].verdict().unwrap().committed_hashes(),
        table.clients[0].verdict().unwrap().committed_hashes()
    );
    assert_eq!(
        table.clients[0].state().unwrap().unwrap().moves,
        table.clients[1].state().unwrap().unwrap().moves
    );
}

async fn play(table: &mut Table, by: usize, uci: &str) {
    propose(
        table,
        by,
        Body::Move {
            uci: uci.into(),
            san: None,
        },
        THINK,
        RESPOND,
    )
    .await;
}

#[tokio::test]
async fn two_players_and_a_watchman_play_to_mate() {
    let mut table = table(120_000).await;

    let err = table.clients[0]
        .propose_body(&Body::Move {
            uci: "e2e5".into(),
            san: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("illegal"),
        "White's client refuses before writing: {err}"
    );
    let acts = table.clients[1].act().await.unwrap();
    assert!(
        acts.iter().all(|a| !matches!(a, Action::Confirmed(_))),
        "Black is not asked to confirm a move that was never proposed: {acts:?}"
    );
    assert_eq!(table.clients[0].verdict().unwrap().committed.len(), 1);

    play(&mut table, 0, "e2e4").await;
    propose(&mut table, 0, Body::OfferDraw {}, THINK, RESPOND).await;
    assert_eq!(
        table.clients[1].state().unwrap().unwrap().draw_offer,
        Some(0),
        "Black confirmed that the offer was made, not that the game is drawn"
    );
    assert!(table.clients[0].state().unwrap().unwrap().result.is_none());
    play(&mut table, 1, "e7e5").await;
    assert!(
        table.clients[0]
            .state()
            .unwrap()
            .unwrap()
            .draw_offer
            .is_none(),
        "moving declines the offer"
    );

    for (by, uci) in [
        (0, "d1h5"),
        (1, "b8c6"),
        (0, "f1c4"),
        (1, "g8f6"),
        (0, "h5f7"),
    ] {
        play(&mut table, by, uci).await;
    }

    for c in &table.clients {
        let state = c.state().unwrap().unwrap();
        assert_eq!(state.result.as_deref(), Some("1-0"));
        assert_eq!(state.moves.len(), 7);
        assert_eq!(
            chess_view(&state).unwrap().san.last().map(String::as_str),
            Some("Qxf7#")
        );
        assert!(chess_view(&state).unwrap().legal_uci.is_empty());
        let page = chain_view(&Chess, c.verdict().unwrap(), &[]);
        let moves: Vec<_> = page.committed.iter().filter(|l| l.kind == "move").collect();
        assert_eq!(moves.len(), 7);
        assert!(moves.iter().all(|l| l.think.ms == Some(THINK)));
        assert!(moves.iter().all(|l| l.respond.ms == Some(RESPOND)));
        assert!(page.committed[0].think.ms.is_none());
    }
    let err = table.clients[1]
        .propose_body(&Body::Move {
            uci: "a7a6".into(),
            san: None,
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("over") || err.to_string().contains("may not"));

    let close = table.clients[1]
        .propose_close(CloseReason::Finished)
        .await
        .unwrap();
    table.clients[0].sync().await.unwrap();
    table.clients[0].confirm(close).await.unwrap();
    table.dog.poll().await.unwrap();
    for c in &mut table.clients {
        c.act().await.unwrap();
    }
    for c in &table.clients {
        match &c.verdict().unwrap().status {
            Status::Closed(outcome) => {
                assert_eq!(outcome.summary, "1-0");
                assert_eq!(outcome.winners, vec![0]);
            }
            other => panic!("expected a finished game, got {other:?}"),
        }
    }

    {
        let mut storage = table.shared.lock().unwrap();
        let dog_folder = format!(
            "pubky://{}{}",
            table.dog.store().me(),
            table.dog.signer().path()
        );
        storage.folders.remove(&dog_folder);
    }
    let observer = MemoryStore::new("", Arc::clone(&table.shared));
    let seen = verify_from(
        &Chess,
        &observer,
        table.clients[0].chain(),
        (
            table.clients[0].store().me().to_string(),
            table.clients[0].signer().path(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        seen.verdict.committed_hashes(),
        table.clients[0].verdict().unwrap().committed_hashes()
    );
    let page = chain_view(&Chess, &seen.verdict, &[]);
    let mate = page
        .committed
        .iter()
        .find(|l| l.kind == "move" && l.body["uci"] == "h5f7")
        .unwrap();
    assert_eq!(mate.think.ms, Some(THINK));
    assert_eq!(mate.respond.ms, Some(RESPOND));
    assert!(mate.observed_at.ms.is_some());
    assert_eq!(page.state.as_ref().unwrap()["result"], "1-0");
}

#[tokio::test]
async fn white_loses_on_time_when_the_watchman_has_seen_the_silence() {
    let mut table = table(60_000).await;
    table.clock.advance(61_000);
    let close = table.clients[1].propose_abandoned(&[0]).await.unwrap();
    for c in &mut table.clients {
        c.sync().await.unwrap();
    }
    for c in &table.clients {
        assert_eq!(
            c.verdict().unwrap().status,
            Status::Paused {
                seq: 1,
                close: CloseState::Asserted,
            },
            "without the receipt the loss is only asserted"
        );
        assert!(!c.verdict().unwrap().is_final());
    }
    table.dog.poll().await.unwrap();
    for c in &mut table.clients {
        c.sync().await.unwrap();
    }
    for c in &table.clients {
        match &c.verdict().unwrap().status {
            Status::Abandoned { subjects, outcome } => {
                assert_eq!(subjects, &vec![0]);
                assert_eq!(outcome.summary, "0-1");
                assert_eq!(outcome.winners, vec![1]);
            }
            other => panic!("expected White's time loss, got {other:?}"),
        }
        assert!(c.verdict().unwrap().is_final());
        assert_eq!(c.verdict().unwrap().committed_hashes().last(), Some(&close));
    }
    let err = table.clients[0]
        .propose_body(&Body::Move {
            uci: "e2e4".into(),
            san: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("no open") || err.to_string().contains("closed"),
        "{err}"
    );
}
