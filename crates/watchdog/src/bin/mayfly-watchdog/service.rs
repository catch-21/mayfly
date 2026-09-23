//! The sweep loop (§16.2.1 E): `Operator::sweep()` every interval until told to stop.
//!
//! A sweep that fails is logged and tried again next interval; a homeserver that is down for
//! a minute must not take the service with it. What each sweep did is logged at `info` when
//! it did anything, and recorded for `/status` either way.

use std::future::Future;
use std::time::Duration;

use tracing::{debug, info, warn};

use pubky_mayfly_client::{Error, Signer, Store};
use pubky_mayfly_watchdog::{Operator, SweepReport};

use crate::health::Status;

/// An operator, its customers, and the status it reports into.
pub struct Service<S: Store + Clone, K: Signer + Clone> {
    op: Operator<S, K>,
    customers: Vec<String>,
    status: Status,
    interval: Duration,
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
        }
    }

    /// One sweep, logged and recorded.
    pub async fn run_once(&mut self) -> Result<SweepReport, Error> {
        match self.op.sweep().await {
            Ok(report) => {
                let quiet = report.engaged.is_empty()
                    && report.extended.is_empty()
                    && report.lapsed.is_empty()
                    && report.declined.is_empty()
                    && report.receipts == 0;
                if quiet {
                    debug!(watching = self.op.watching().count(), "sweep: nothing new");
                } else {
                    info!(
                        engaged = ?report.engaged,
                        extended = ?report.extended,
                        lapsed = ?report.lapsed,
                        declined = ?report.declined,
                        receipts = report.receipts,
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
        }
    }

    /// Sweep until `stop` resolves. Errors are not fatal.
    pub async fn run<F: Future<Output = ()>>(mut self, stop: F) {
        let mut stop = std::pin::pin!(stop);
        loop {
            let _ = self.run_once().await;
            tokio::select! {
                () = &mut stop => return,
                () = tokio::time::sleep(self.interval) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pubky_common::crypto::Keypair;

    use pubky_mayfly::sim::Tally;
    use pubky_mayfly_client::{ChainClient, GenesisSpec, LocalSigner, MemoryStore, Signer};
    use pubky_mayfly_watchdog::{Operator, Terms};

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

    /// The loop body over the in-memory store: a customer's chain naming the watchdog is
    /// engaged on the first sweep, and the status page shows it.
    #[tokio::test]
    async fn run_once_engages_and_reports() {
        let shared = MemoryStore::shared();
        let (dog_store, dog_signer) = actor(&shared, "watchdog.example");
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

        // Alice starts a chain with Bob naming the watchdog; her client writes the marker.
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
    }

    /// `run` stops when told to, having swept at least once.
    #[tokio::test]
    async fn run_stops_on_signal() {
        let shared = MemoryStore::shared();
        let (dog_store, dog_signer) = actor(&shared, "watchdog.example");
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
