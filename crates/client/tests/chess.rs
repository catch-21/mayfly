//! A chess game (`chess/1`) run the way the app runs it: both sides call `act` except to
//! propose a move. A legal move is confirmed only after shakmaty has accepted it. An illegal
//! move written straight into a folder is rejected and never committed.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pubky_common::crypto::Keypair;
use serde_json::json;

use pubky_mayfly::hash::Hash;
use pubky_mayfly::record::Link;
use pubky_mayfly::sim::Storage;
use pubky_mayfly::{typ, PROTOCOL_VERSION};
use pubky_mayfly_client::layout::Folder;
use pubky_mayfly_client::signer::sign_record;
use pubky_mayfly_client::{
    Action, ChainClient, GenesisSpec, LocalSigner, MemoryStore, Signer, Store,
};
use pubky_mayfly_rules::chess::{Body, Chess};

type Client = ChainClient<Chess, MemoryStore, LocalSigner>;

const NOW_S: u64 = 1_757_779_812;
const THINK_MS: u64 = 60_000;

#[derive(Clone)]
struct Clock(Arc<AtomicU64>);

impl Clock {
    fn reader(&self) -> impl Fn() -> u64 + Send + Sync + 'static {
        let c = self.clone();
        move || c.0.load(Ordering::SeqCst)
    }
    fn advance(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

fn party(shared: &Arc<Mutex<Storage>>, app: &str) -> (LocalSigner, MemoryStore) {
    let identity = Keypair::random();
    let signer = LocalSigner::mint(&identity, app, NOW_S, 365 * 24 * 3600);
    let store = MemoryStore::new(identity.public_key().z32(), Arc::clone(shared));
    (signer, store)
}

async fn everyone(clients: &mut [Client]) -> Vec<Vec<Action>> {
    let mut out = Vec::new();
    for c in clients.iter_mut() {
        out.push(c.act().await.unwrap());
    }
    out
}

async fn settle(clients: &mut [Client], clock: &Clock, len: usize) {
    for _ in 0..12 {
        everyone(clients).await;
        if clients[0].verdict().unwrap().committed.len() >= len {
            return;
        }
        clock.advance(THINK_MS + 1);
    }
    panic!(
        "did not settle at {len}: {:?}",
        clients[0].verdict().unwrap().committed.len()
    );
}

async fn opened() -> (Vec<Client>, Clock) {
    let shared = MemoryStore::shared();
    let clock = Clock(Arc::new(AtomicU64::new(NOW_S * 1000)));
    let parties = [
        party(&shared, "chess.example"),
        party(&shared, "chess.example"),
    ];
    let pubkies: Vec<String> = parties.iter().map(|(s, _)| s.pubky()).collect();
    let mut spec = GenesisSpec::new(pubkies).with_apps(&["chess.example", "chess.example"]);
    spec.roles = vec![Some("white".into()), Some("black".into())];
    spec.options = json!({ "time_control": { "think_ms": THINK_MS, "respond_ms": THINK_MS } });
    let alice = ChainClient::create(Chess, parties[0].1.clone(), parties[0].0.clone(), spec)
        .await
        .unwrap()
        .with_clock(clock.reader());
    let invite = alice.invite_url();
    let mut bob = ChainClient::open_url(Chess, parties[1].1.clone(), parties[1].0.clone(), &invite)
        .unwrap()
        .with_clock(clock.reader());
    bob.sync().await.unwrap();
    bob.join().await.unwrap();
    let mut clients = vec![alice, bob];
    settle(&mut clients, &clock, 1).await;
    (clients, clock)
}

#[tokio::test]
async fn a_game_ends_in_mate_and_an_unwatched_move_is_not_timed() {
    let (mut clients, clock) = opened().await;
    let moves = [
        (0, "e2e4"),
        (1, "e7e5"),
        (0, "d1h5"),
        (1, "b8c6"),
        (0, "f1c4"),
        (1, "g8f6"),
        (0, "h5f7"),
    ];
    for (by, uci) in moves {
        let len = clients[0].verdict().unwrap().committed.len();
        clients[by]
            .propose_body(&Body::Move {
                uci: uci.into(),
                san: None,
            })
            .await
            .unwrap();
        settle(&mut clients, &clock, len + 1).await;
    }
    let state = clients[0].state().unwrap().unwrap();
    assert_eq!(state.result.as_deref(), Some("1-0"));
    assert_eq!(state.moves.len(), 7);
    let mate = clients[0].verdict().unwrap().committed.last().unwrap();
    assert!(
        mate.think.ms.is_none() && mate.observed_at.ms.is_none(),
        "no watchman, so the move is committed and not timed"
    );
    let close = clients[1]
        .propose_close(pubky_mayfly::record::CloseReason::Finished)
        .await
        .unwrap();
    clients[0].sync().await.unwrap();
    clients[0].confirm(close).await.unwrap();
    settle(&mut clients, &clock, 9).await;
    assert_eq!(
        clients[0].state().unwrap().unwrap().result.as_deref(),
        Some("1-0")
    );
}

#[tokio::test]
async fn the_opponent_rejects_an_illegal_move_written_into_the_folder() {
    let (mut clients, _clock) = opened().await;
    let chain = clients[0].chain().clone();
    let author = clients[0].signer().pubky();
    let kid = clients[0].signer().kid();
    let path_prefix = clients[0].signer().path();
    let (seq, prev, state, confirms) = {
        let verdict = clients[0].verdict().unwrap();
        let head = verdict.head().unwrap();
        let open = verdict.open.as_ref().unwrap();
        let mut confirms: Vec<String> = head
            .qc
            .iter()
            .map(|c| String::from_utf8(c.bytes.clone()).unwrap())
            .collect();
        confirms.sort();
        (
            open.seq,
            head.link.hash.to_base64url(),
            head.link.payload.state.clone(),
            confirms,
        )
    };
    let link = Link {
        v: PROTOCOL_VERSION,
        chain,
        seq,
        round: 0,
        prev,
        confirms,
        receipts: Vec::new(),
        author,
        kid,
        ts: 0,
        kind: "move".into(),
        body: json!({ "uci": "e2e5" }),
        state,
        grant: None,
    };
    let bytes = sign_record(clients[0].signer(), typ::LINK, &link)
        .await
        .unwrap();
    let hash = Hash::of(&bytes);
    let path = Folder::from_path(&path_prefix)
        .chain(clients[0].chain())
        .link(seq, &hash);
    clients[0].store().put(&path, bytes).await.unwrap();

    let actions = clients[1].act().await.unwrap();
    assert!(
        actions.iter().any(|a| matches!(a, Action::Rejected { .. })),
        "the opponent refuses the illegal move: {actions:?}"
    );
    assert_eq!(
        clients[1].verdict().unwrap().committed.len(),
        1,
        "the illegal move is not in the chain"
    );
}
