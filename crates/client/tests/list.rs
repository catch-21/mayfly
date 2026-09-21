//! A shared list (`list/1`, §10.1) run the way an app would run it: parties join from an
//! invite URL, propose typed bodies, and otherwise only call [`ChainClient::act`] whenever
//! anything changes. The honest choreography of §8.2 — confirming, re-proposing after a dead
//! round, skipping a silent proposer, passing — happens inside `act`; what needs a person (a
//! `close`) comes back as a decision.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pubky_common::crypto::Keypair;
use serde_json::json;

use pubky_mayfly::fold::Status;
use pubky_mayfly::record::CloseReason;
use pubky_mayfly::sim::Storage;
use pubky_mayfly::vote::designated;
use pubky_mayfly_client::layout::Folder;
use pubky_mayfly_client::{
    Action, ChainClient, GenesisSpec, LocalSigner, MemoryStore, Signer, Store,
};
use pubky_mayfly_rules::list::{Body, List};

type Client = ChainClient<List, MemoryStore, LocalSigner>;

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

/// Everyone present acts once; the actions, per party.
async fn everyone(clients: &mut [Client], present: &[usize]) -> Vec<Vec<Action>> {
    let mut out = vec![Vec::new(); clients.len()];
    for &i in present {
        out[i] = clients[i].act().await.unwrap();
    }
    out
}

/// Act until the chain has `len` committed links or `limit` rounds of acting pass, advancing
/// the clock past `think_ms` each round so that skips can happen.
async fn settle(
    clients: &mut [Client],
    present: &[usize],
    clock: &Clock,
    len: usize,
) -> Vec<Action> {
    let mut seen = Vec::new();
    for _ in 0..12 {
        for acts in everyone(clients, present).await {
            seen.extend(acts);
        }
        if clients[present[0]].verdict().unwrap().committed.len() >= len {
            return seen;
        }
        clock.advance(THINK_MS + 1);
    }
    panic!("did not settle: {seen:?}");
}

fn items(c: &Client) -> Vec<String> {
    c.state()
        .unwrap()
        .unwrap()
        .items
        .iter()
        .map(|i| i.text.clone())
        .collect()
}

#[tokio::test]
async fn a_shared_list_runs_on_act_alone() {
    let shared = MemoryStore::shared();
    let clock = Clock(Arc::new(AtomicU64::new(NOW_S * 1000)));
    let parties = [
        party(&shared, "list.example"),
        party(&shared, "list.example"),
        party(&shared, "other.example"),
    ];
    let pubkies: Vec<String> = parties.iter().map(|(s, _)| s.pubky()).collect();

    // Alice creates the list and sends an invite URL; Bob and Carol join from it.
    let mut spec = GenesisSpec::new(pubkies.clone()).with_apps(&[
        "list.example",
        "list.example",
        "other.example",
    ]);
    spec.options = json!({ "time_control": { "think_ms": THINK_MS, "respond_ms": THINK_MS } });
    let alice = ChainClient::create(List, parties[0].1.clone(), parties[0].0.clone(), spec)
        .await
        .unwrap()
        .with_clock(clock.reader());
    let invite = alice.invite_url();
    assert!(invite.starts_with(&format!(
        "pubky://{}/pub/list.example/mayfly/chains/",
        pubkies[0]
    )));
    let mut clients = vec![alice];
    for (signer, store) in &parties[1..] {
        let mut c = ChainClient::open_url(List, store.clone(), signer.clone(), &invite)
            .unwrap()
            .with_clock(clock.reader());
        c.sync().await.unwrap();
        c.join().await.unwrap();
        clients.push(c);
    }
    let all = [0, 1, 2];
    settle(&mut clients, &all, &clock, 1).await;
    for c in &clients {
        assert_eq!(c.verdict().unwrap().status, Status::Ongoing);
        assert_eq!(items(c), Vec::<String>::new());
    }

    // An append: the author proposes a typed body; the others confirm inside `act`.
    let add = |id: &str, text: &str| Body::Add {
        id: id.into(),
        text: text.into(),
        qty: None,
    };
    let l1 = clients[0].propose_body(&add("milk", "Milk")).await.unwrap();
    let acts = settle(&mut clients, &all, &clock, 2).await;
    assert_eq!(
        acts.iter().filter(|a| **a == Action::Confirmed(l1)).count(),
        2
    );
    for c in &clients {
        assert_eq!(items(c), vec!["Milk"]);
    }

    // Competing proposals converge through a dead round, and a proposer who walks away is
    // skipped after `think_ms`. The walker is whoever round 1 would fall to, so the skip is
    // exercised, not left to chance.
    let seq = clients[0].verdict().unwrap().open.as_ref().unwrap().seq;
    let walker = designated(clients[0].chain(), seq, 1, 3);
    let present: Vec<usize> = all.iter().copied().filter(|p| *p != walker).collect();
    let (a, b) = (present[0], present[1]);
    let la = clients[a].propose_body(&add("eggs", "Eggs")).await.unwrap();
    let lb = clients[b]
        .propose_body(&add("bread", "Bread"))
        .await
        .unwrap();
    // Round 0 dies; round 1 is the walker's and nobody hears from them; after `think_ms` the
    // others skip, round 2 falls to one of them, who re-proposes the lowest-hash content, and
    // the other confirms it. Under unanimity that is as far as two of three can go.
    let mut acts = Vec::new();
    for _ in 0..4 {
        for a in everyone(&mut clients, &present).await {
            acts.extend(a);
        }
        clock.advance(THINK_MS + 1);
    }
    assert!(
        acts.iter().any(|x| matches!(x, Action::Skipped(_))),
        "{acts:?}"
    );
    assert!(
        acts.iter()
            .any(|x| matches!(x, Action::Reproposed { earlier, .. } if *earlier == la.min(lb))),
        "the lowest-hash content was re-proposed: {acts:?}"
    );
    assert_eq!(
        clients[a].verdict().unwrap().committed.len(),
        2,
        "one vote short"
    );
    // The walker was slow, not gone: on return, `act` confirms and the append commits.
    let back = clients[walker].act().await.unwrap();
    assert!(
        matches!(back.as_slice(), [Action::Confirmed(_)]),
        "{back:?}"
    );
    settle(&mut clients, &all, &clock, 3).await;
    let winner = if la < lb { "Eggs" } else { "Bread" };
    for c in &clients {
        assert_eq!(items(c), vec!["Milk", winner]);
    }
    // The other author simply proposes again.
    let loser = if la < lb { b } else { a };
    clients[loser]
        .propose_body(&add(
            if winner == "Eggs" { "bread" } else { "eggs" },
            if winner == "Eggs" { "Bread" } else { "Eggs" },
        ))
        .await
        .unwrap();
    settle(&mut clients, &all, &clock, 4).await;
    for c in &clients {
        assert_eq!(items(c).len(), 3);
        let v = c.verdict().unwrap();
        assert!(
            v.anomalies
                .iter()
                .all(|x| x.kind != pubky_mayfly::fold::AnomalyKind::InvalidSkip),
            "{:?}",
            v.anomalies
        );
    }

    // Ticking and archiving are ordinary appends; closing is not: `act` puts a close to the
    // user and the app confirms it.
    clients[1]
        .propose_body(&Body::Tick { id: "milk".into() })
        .await
        .unwrap();
    settle(&mut clients, &all, &clock, 5).await;
    assert!(clients[2].state().unwrap().unwrap().items[0].ticked);
    let close = clients[2].propose_close(CloseReason::Agreed).await.unwrap();
    let acts = everyone(&mut clients, &all).await;
    for &p in &[0, 1] {
        assert!(
            matches!(
                acts[p].as_slice(),
                [Action::Decision { candidate, repropose: false, .. }] if candidate.hash == close
            ),
            "{:?}",
            acts[p]
        );
        clients[p].confirm(close).await.unwrap();
    }
    settle(&mut clients, &all, &clock, 6).await;
    for c in &clients {
        assert!(c.verdict().unwrap().is_final());
        assert!(everyone_is_done(c).await);
    }
    // The marker moved to `index/finished/` on every party's storage.
    for (signer, store) in &parties {
        let folder = Folder::from_path(&signer.path());
        let chain = clients[0].chain();
        assert!(store
            .get(store.me(), &folder.active(chain))
            .await
            .unwrap()
            .is_none());
        assert!(store
            .get(store.me(), &folder.finished(chain))
            .await
            .unwrap()
            .is_some());
    }
}

async fn everyone_is_done(c: &Client) -> bool {
    c.verdict().unwrap().open.is_none()
}
