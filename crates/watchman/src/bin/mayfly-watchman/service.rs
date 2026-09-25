//! The loop (§16.2.1 E): `Operator::sweep()` every interval, and, between sweeps, a poll of
//! any chain whose folders a homeserver has just reported changed.
//!
//! Two sources drive the operator. The homeservers' event streams say *which* folder changed
//! the moment it does, so a new record is receipted within a second of being written rather
//! than at the next sweep; the sweep every interval is the fallback that reads everything
//! anyway, finds new markers, renews engagements, and now and then audits. A stream that
//! cannot be opened, or that drops, costs nothing but that promptness: the sweep covers it,
//! and the next sweep tries to open it again.
//!
//! A sweep that fails is logged and tried again next interval; a homeserver that is down for
//! a minute must not take the service with it. What each sweep did is logged at `info` when
//! it did anything, and recorded for `/status` either way.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::time::Duration;

use futures_util::StreamExt;
use pubky::{Pubky, PublicKey};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use pubky_mayfly::hash::ChainId;
use pubky_mayfly_client::{Error, Signer, Store};
use pubky_mayfly_watchman::{Operator, SweepReport};

use crate::health::Status;

/// How long to wait after an event before polling, so a burst of writes — a link and the
/// mirrors that follow it — is one poll, not one per file.
const SETTLE: Duration = Duration::from_millis(300);

/// One folder change, as a stream reported it.
#[derive(Debug, Clone)]
struct Changed {
    owner: String,
    /// Absolute path of the file written or deleted.
    path: String,
}

/// An operator, its customers, and the status it reports into.
pub struct Service<S: Store + Clone, K: Signer + Clone> {
    pub(crate) op: Operator<S, K>,
    customers: Vec<String>,
    status: Status,
    interval: Duration,
    events: Option<Events>,
}

/// The event streams: one task per `(owner, protocol folder)` among the watched chains'
/// folders, each forwarding the paths it hears into one channel.
struct Events {
    pubky: Pubky,
    tx: mpsc::UnboundedSender<Changed>,
    rx: mpsc::UnboundedReceiver<Changed>,
    streams: BTreeMap<(String, String), JoinHandle<()>>,
}

impl<S: Store + Clone, K: Signer + Clone> Service<S, K> {
    /// Sweep `op` every `sweep_secs`, reporting into `status`. `customers` is every pubky
    /// `op` was given, for the status page: the operator itself does not list them.
    pub fn new(
        op: Operator<S, K>,
        customers: Vec<String>,
        status: Status,
        sweep_secs: u64,
    ) -> Self {
        Self {
            op,
            customers,
            status,
            interval: Duration::from_secs(sweep_secs.max(1)),
            events: None,
        }
    }

    /// Also follow the parties' homeserver event streams through `pubky`, polling a chain as
    /// soon as one of its folders changes.
    pub fn with_events(mut self, pubky: Pubky) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        self.events = Some(Events {
            pubky,
            tx,
            rx,
            streams: BTreeMap::new(),
        });
        self
    }

    /// One sweep, logged and recorded; then the streams are brought in line with what is
    /// watched.
    pub async fn run_once(&mut self) -> Result<SweepReport, Error> {
        let out = match self.op.sweep().await {
            Ok(report) => {
                if report.is_quiet() {
                    debug!(watching = self.op.watching().count(), "sweep: nothing new");
                } else {
                    info!(
                        engaged = ?report.engaged,
                        resumed = ?report.resumed,
                        extended = ?report.extended,
                        lapsed = ?report.lapsed,
                        declined = ?report.declined,
                        receipts = report.receipts,
                        timed_out = ?report.timed_out,
                        failed = ?report.failed,
                        slow_customers = ?report.slow_customers,
                        audited = report.audited,
                        watching = self.op.watching().count(),
                        "sweep"
                    );
                }
                self.status.record_sweep(&self.op, &self.customers, &report);
                Ok(report)
            }
            Err(e) => {
                warn!(error = %e, "sweep failed; trying again next interval");
                self.status.record_error(&e.to_string());
                Err(e)
            }
        };
        self.sync_streams();
        out
    }

    /// Sweep every interval and poll on events until `stop` resolves. Errors are not fatal.
    pub async fn run<F: Future<Output = ()>>(mut self, stop: F) {
        let mut stop = std::pin::pin!(stop);
        let mut ticker = tokio::time::interval(self.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let event = async {
                match self.events.as_mut() {
                    Some(ev) => ev.rx.recv().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                () = &mut stop => break,
                _ = ticker.tick() => {
                    let _ = self.run_once().await;
                }
                Some(first) = event => {
                    self.on_events(first).await;
                }
            }
        }
        if let Some(ev) = self.events.take() {
            for (_, task) in ev.streams {
                task.abort();
            }
        }
    }

    /// Gather the burst that follows `first`, then poll each chain those folders belong to.
    async fn on_events(&mut self, first: Changed) {
        let mut changed = vec![first];
        tokio::time::sleep(SETTLE).await;
        if let Some(ev) = self.events.as_mut() {
            while let Ok(c) = ev.rx.try_recv() {
                changed.push(c);
            }
        }
        // Which chains: the folder (owner, protocol folder) each path sits under, and the
        // chain id in the path if there is one — a folder shared by several chains' parties
        // under one app is told apart by the `chains/<id>/` segment.
        let mut chains: BTreeSet<ChainId> = BTreeSet::new();
        for c in &changed {
            for chain in self.op.chains().cloned().collect::<Vec<_>>() {
                let belongs = self.op.folders_of(&chain).iter().any(|(owner, folder)| {
                    *owner == c.owner && c.path.starts_with(folder.as_str())
                });
                if !belongs {
                    continue;
                }
                if let Some(id) = chain_in_path(&c.path) {
                    if id != chain {
                        continue;
                    }
                }
                self.op.mark_changed(&chain, &c.owner, &c.path);
                chains.insert(chain);
            }
        }
        for chain in chains {
            match self.op.poll_chain(&chain).await {
                Ok(Some(report)) => {
                    if report.receipts > 0 || !report.inconsistent.is_empty() {
                        info!(%chain, receipts = report.receipts, inconsistent = report.inconsistent.len(), "event poll");
                    }
                    self.status.record_event_poll(&self.op, report.receipts);
                }
                Ok(None) => {}
                Err(e) => {
                    warn!(%chain, error = %e, "event poll failed; the sweep will cover it");
                    self.status.record_event_poll(&self.op, 0);
                }
            }
        }
    }

    /// Open a stream for every `(owner, protocol folder)` a watched chain reads, and close the
    /// ones no watched chain reads any more. A stream task that has ended (the homeserver
    /// closed it, or would not open it) is dropped here and opened afresh.
    fn sync_streams(&mut self) {
        let Some(ev) = self.events.as_mut() else {
            return;
        };
        let mut wanted: BTreeSet<(String, String)> = BTreeSet::new();
        for chain in self.op.chains() {
            for folder in self.op.folders_of(chain) {
                wanted.insert(folder);
            }
        }
        ev.streams.retain(|key, task| {
            let keep = wanted.contains(key) && !task.is_finished();
            if !keep {
                task.abort();
            }
            keep
        });
        for key in wanted {
            if ev.streams.contains_key(&key) {
                continue;
            }
            let (owner, folder) = key.clone();
            let Ok(pk) = PublicKey::try_from(owner.as_str()) else {
                continue;
            };
            let pubky = ev.pubky.clone();
            let tx = ev.tx.clone();
            // Records of every chain under this app folder arrive on one stream.
            let prefix = format!("{folder}chains/");
            let task = tokio::spawn(async move {
                let stream = match pubky
                    .event_stream_for_user(&pk, None)
                    .path(prefix.clone())
                    .live()
                    .subscribe()
                    .await
                {
                    Ok(s) => s,
                    Err(e) => {
                        debug!(%owner, %prefix, error = %e, "no event stream; the sweep covers it");
                        return;
                    }
                };
                debug!(%owner, %prefix, "following events");
                let mut stream = stream;
                while let Some(item) = stream.next().await {
                    match item {
                        Ok(event) => {
                            let changed = Changed {
                                owner: owner.clone(),
                                path: event.resource.path.as_str().to_string(),
                            };
                            if tx.send(changed).is_err() {
                                return;
                            }
                        }
                        Err(e) => {
                            debug!(%owner, %prefix, error = %e, "event stream ended; reopened next sweep");
                            return;
                        }
                    }
                }
            });
            ev.streams.insert(key, task);
        }
        self.status.record_streams(ev.streams.len());
    }
}

/// The `<chain_id>` in `…/chains/<chain_id>/…`, if the path has one.
fn chain_in_path(path: &str) -> Option<ChainId> {
    let at = path.find("chains/")?;
    let rest = &path[at + "chains/".len()..];
    let id = rest.split('/').next()?;
    ChainId::parse(id).ok()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pubky_common::crypto::Keypair;

    use pubky_mayfly::sim::Tally;
    use pubky_mayfly_client::{ChainClient, GenesisSpec, LocalSigner, MemoryStore, Signer};
    use pubky_mayfly_watchman::{Operator, Terms};

    use super::*;
    use crate::health::Identity;

    const NOW_S: u64 = 1_757_779_812;
    const TWO_YEARS: u64 = 2 * 365 * 24 * 3600;

    fn actor(
        shared: &Arc<std::sync::Mutex<pubky_mayfly::sim::Storage>>,
        app: &str,
    ) -> (MemoryStore, LocalSigner) {
        let identity = Keypair::random();
        let signer = LocalSigner::mint(&identity, app, NOW_S, TWO_YEARS);
        let store = MemoryStore::new(identity.public_key().z32(), Arc::clone(shared));
        (store, signer)
    }

    fn identity(signer: &LocalSigner) -> Identity {
        Identity {
            pubky: signer.pubky(),
            kid: signer.kid(),
            client_id: signer.client_id(),
            network: "memory".into(),
            homeserver: String::new(),
            path: signer.path(),
        }
    }

    #[test]
    fn a_chain_id_is_read_from_a_record_path() {
        let id = ChainId::derive(b"g");
        assert_eq!(
            chain_in_path(&format!(
                "/pub/list.example/mayfly/chains/{id}/links/00000001-x.jws"
            )),
            Some(id.clone())
        );
        assert_eq!(
            chain_in_path(&format!("/pub/list.example/mayfly/chains/{id}")),
            Some(id)
        );
        assert_eq!(
            chain_in_path("/pub/list.example/mayfly/index/active/x"),
            None
        );
        assert_eq!(
            chain_in_path("/pub/list.example/mayfly/chains/NOPE/x"),
            None
        );
    }

    /// A sweep engages from a marker, receipts, and reports; `/status` follows.
    #[tokio::test]
    async fn run_once_engages_and_reports() {
        let shared = MemoryStore::shared();
        let (dog_store, dog_signer) = actor(&shared, "watchman.example");
        let (alice_store, alice_signer) = actor(&shared, "chess.example");
        let (bob_store, bob_signer) = actor(&shared, "notes.example");

        let mut op = Operator::new(dog_store, dog_signer.clone(), Terms::receipts(0))
            .with_clock(|| NOW_S * 1000)
            .engage_for(3600)
            .renew_before(60);
        op.free(alice_signer.pubky());
        let customers = vec![alice_signer.pubky()];
        let status = Status::new(identity(&dog_signer), 15);
        let mut service = Service::new(op, customers, status.clone(), 15);

        // Nothing to do: a quiet sweep is still a healthy one.
        let report = service.run_once().await.unwrap();
        assert_eq!(report, SweepReport::default());
        assert!(status.healthy());
        assert_eq!(status.snapshot().sweeps, 1);
        assert_eq!(
            status.snapshot().customers,
            vec![crate::health::Customer {
                pubky: alice_signer.pubky(),
                credit: crate::health::CreditView::Free,
            }]
        );

        // Alice starts a chain with Bob naming the watchman; her client writes the marker.
        let mut spec = GenesisSpec::new(vec![alice_signer.pubky(), bob_signer.pubky()])
            .with_apps(&["chess.example", "notes.example"]);
        spec.witnesses = vec![dog_signer.pubky()];
        let alice = ChainClient::create(Tally, alice_store, alice_signer.clone(), spec)
            .await
            .unwrap();
        let chain = alice.chain().clone();
        let mut bob = ChainClient::open(
            Tally,
            bob_store,
            bob_signer,
            chain.clone(),
            (alice_signer.pubky(), alice_signer.path()),
        );
        bob.sync().await.unwrap();
        bob.join().await.unwrap();

        let report = service.run_once().await.unwrap();
        assert_eq!(report.engaged, vec![chain.clone()]);
        assert_eq!(report.receipts, 2, "genesis and Bob's confirmation");
        let snap = status.snapshot();
        assert_eq!(snap.sweeps, 2);
        assert_eq!(snap.errors, 0);
        assert_eq!(snap.watching.len(), 1);
        assert_eq!(snap.watching[0].chain, chain.to_string());
        assert_eq!(snap.watching[0].until, NOW_S + 3600);
        assert_eq!(snap.watching[0].receipts, 2);
        assert_eq!(snap.last_sweep.unwrap().engaged, vec![chain.to_string()]);
        assert!(snap.last_sweep_at.is_some());
        assert_eq!(service.op.watching().count(), 1);
        assert_eq!(snap.streams, 0, "no event source configured");
    }

    /// An event naming a watched folder makes the service poll that chain at once, and only
    /// that chain; the status page counts it.
    #[tokio::test]
    async fn an_event_polls_the_chain_it_names() {
        let shared = MemoryStore::shared();
        let (dog_store, dog_signer) = actor(&shared, "watchman.example");
        let (alice_store, alice_signer) = actor(&shared, "chess.example");
        let (bob_store, bob_signer) = actor(&shared, "notes.example");
        let mut op = Operator::new(dog_store, dog_signer.clone(), Terms::receipts(0))
            .with_clock(|| NOW_S * 1000)
            .engage_for(3600);
        op.free(alice_signer.pubky());
        let status = Status::new(identity(&dog_signer), 15);
        let mut service = Service::new(op, vec![alice_signer.pubky()], status.clone(), 15);
        // Events without a Pubky facade: the channel alone, fed by the test.
        let (tx, rx) = mpsc::unbounded_channel();
        service.events = Some(Events {
            pubky: Pubky::testnet().unwrap(),
            tx: tx.clone(),
            rx,
            streams: BTreeMap::new(),
        });

        let mut spec = GenesisSpec::new(vec![alice_signer.pubky(), bob_signer.pubky()])
            .with_apps(&["chess.example", "notes.example"]);
        spec.witnesses = vec![dog_signer.pubky()];
        let mut alice = ChainClient::create(Tally, alice_store, alice_signer.clone(), spec)
            .await
            .unwrap();
        let chain = alice.chain().clone();
        let mut bob = ChainClient::open(
            Tally,
            bob_store,
            bob_signer.clone(),
            chain.clone(),
            (alice_signer.pubky(), alice_signer.path()),
        );
        bob.sync().await.unwrap();
        bob.join().await.unwrap();
        let report = service.run_once().await.unwrap();
        assert_eq!(report.engaged, vec![chain.clone()]);
        let receipts_after_sweep = status.snapshot().watching[0].receipts;

        // Alice appends; Bob's homeserver (his folder) reports his mirror of it. No sweep runs.
        alice.sync().await.unwrap();
        let l1 = alice
            .propose("add", serde_json::json!({ "n": 1 }))
            .await
            .unwrap();
        bob.sync().await.unwrap();
        bob.mirror().await.unwrap();
        let bob_folder = pubky_mayfly_client::layout::Folder::from_path(&bob_signer.path());
        let path = bob_folder.chain(&chain).link(1, &l1);
        service
            .on_events(Changed {
                owner: bob_signer.pubky(),
                path: path.clone(),
            })
            .await;
        let snap = status.snapshot();
        assert_eq!(snap.event_polls, 1);
        assert!(snap.watching[0].receipts > receipts_after_sweep, "{snap:?}");
        assert!(service
            .op
            .watching()
            .next()
            .unwrap()
            .1
            .observed_at(&l1)
            .is_some());

        // A path under a folder nobody watches polls nothing.
        service
            .on_events(Changed {
                owner: "nobody".into(),
                path: "/pub/other.example/mayfly/chains/X/links/y.jws".into(),
            })
            .await;
        assert_eq!(status.snapshot().event_polls, 1);
    }

    /// `run` stops when told to, having swept at least once.
    #[tokio::test]
    async fn run_stops_on_signal() {
        let shared = MemoryStore::shared();
        let (dog_store, dog_signer) = actor(&shared, "watchman.example");
        let op = Operator::new(dog_store, dog_signer.clone(), Terms::receipts(0));
        let status = Status::new(identity(&dog_signer), 15);
        let service = Service::new(op, Vec::new(), status.clone(), 3600);
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let run = tokio::spawn(service.run(async move {
            let _ = rx.await;
        }));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(status.snapshot().sweeps, 1);
        tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), run)
            .await
            .expect("stops promptly")
            .unwrap();
    }
}
