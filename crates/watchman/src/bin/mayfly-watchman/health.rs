//! `GET /healthz` and `GET /status` (§16.2.1 E).
//!
//! `/healthz` is `200` while the last sweep succeeded within three sweep intervals and `503`
//! otherwise, including before the first sweep. `/status` is a JSON snapshot: who the
//! watchman is, what it is watching, its customers and their credit, and the last sweep.

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Json;
use axum::routing::get;
use axum::Router;
use serde::Serialize;

use pubky_mayfly_client::{Signer, Store};
use pubky_mayfly_watchman::{Credit, Operator, SweepReport};

/// Who the watchman is; fixed for the life of the process.
#[derive(Debug, Clone, Serialize)]
pub struct Identity {
    /// The watchman's pubky: what genesis names.
    pub pubky: String,
    /// The Grant client key that signs engagements and receipts.
    pub kid: String,
    /// The app it signed in as.
    pub client_id: String,
    /// The network, as configured.
    pub network: String,
    /// The homeserver pubky.
    pub homeserver: String,
    /// The folder it publishes under: `/pub/<client_id>/mayfly/`.
    pub path: String,
}

/// One engaged chain.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Watching {
    /// The chain id.
    pub chain: String,
    /// End of the current engagement, Unix seconds.
    pub until: u64,
    /// Receipts issued for this chain so far.
    pub receipts: usize,
}

/// A customer's credit (§11.2).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CreditView {
    /// Watched for free.
    Free,
    /// Prepaid watch-time remaining, in seconds.
    Seconds(u64),
}

impl From<Credit> for CreditView {
    fn from(c: Credit) -> Self {
        match c {
            Credit::Free => Self::Free,
            Credit::Seconds(s) => Self::Seconds(s),
        }
    }
}

/// One customer.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Customer {
    /// The customer's pubky.
    pub pubky: String,
    /// What is left.
    pub credit: CreditView,
}

/// A marker not acted on.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeclinedView {
    /// Whose marker.
    pub customer: String,
    /// Which chain.
    pub chain: String,
    /// Why: `NotNamed`, `NoCredit` or `NoGenesis`.
    pub why: String,
}

/// What the last sweep did.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct SweepView {
    /// Chains engaged.
    pub engaged: Vec<String>,
    /// Chains taken back from an earlier process, with their receipts.
    pub resumed: Vec<String>,
    /// Chains renewed.
    pub extended: Vec<String>,
    /// Chains that lapsed.
    pub lapsed: Vec<String>,
    /// Markers declined.
    pub declined: Vec<DeclinedView>,
    /// Receipts written.
    pub receipts: usize,
    /// Chains whose poll ran out of time; tried again next sweep.
    pub timed_out: Vec<String>,
    /// Chains whose poll failed, with the error.
    pub failed: Vec<FailedView>,
    /// Customers whose homeserver did not list `/pub/` in time.
    pub slow_customers: Vec<String>,
    /// Whether this sweep audited (content hashes) rather than polled by name.
    pub audited: bool,
}

/// One chain whose poll failed.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FailedView {
    /// Which chain.
    pub chain: String,
    /// Why.
    pub error: String,
}

impl From<&SweepReport> for SweepView {
    fn from(r: &SweepReport) -> Self {
        let ids = |v: &[pubky_mayfly::hash::ChainId]| v.iter().map(|c| c.to_string()).collect();
        Self {
            engaged: ids(&r.engaged),
            resumed: ids(&r.resumed),
            extended: ids(&r.extended),
            lapsed: ids(&r.lapsed),
            declined: r
                .declined
                .iter()
                .map(|(customer, chain, why)| DeclinedView {
                    customer: customer.clone(),
                    chain: chain.to_string(),
                    why: format!("{why:?}"),
                })
                .collect(),
            receipts: r.receipts,
            timed_out: ids(&r.timed_out),
            failed: r
                .failed
                .iter()
                .map(|(chain, error)| FailedView {
                    chain: chain.to_string(),
                    error: error.clone(),
                })
                .collect(),
            slow_customers: r.slow_customers.clone(),
            audited: r.audited,
        }
    }
}

/// The body of `GET /status`.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    /// Who.
    #[serde(flatten)]
    pub identity: Identity,
    /// Whether `/healthz` would say `200` right now.
    pub healthy: bool,
    /// Engaged chains.
    pub watching: Vec<Watching>,
    /// Customers and credit.
    pub customers: Vec<Customer>,
    /// Sweeps completed, successful or not.
    pub sweeps: u64,
    /// Sweeps that failed.
    pub errors: u64,
    /// When the last successful sweep finished, Unix seconds.
    pub last_sweep_at: Option<u64>,
    /// What it did.
    pub last_sweep: Option<SweepView>,
    /// The last failure, if the most recent sweep failed.
    pub last_error: Option<String>,
    /// Polls made between sweeps because a homeserver reported a folder change.
    pub event_polls: u64,
    /// Receipts those polls wrote.
    pub event_receipts: u64,
    /// Homeserver event streams open right now.
    pub streams: usize,
}

struct Inner {
    identity: Identity,
    watching: Vec<Watching>,
    customers: Vec<Customer>,
    sweeps: u64,
    errors: u64,
    last_sweep_at: Option<u64>,
    last_sweep: Option<SweepView>,
    last_error: Option<String>,
    event_polls: u64,
    event_receipts: u64,
    streams: usize,
    /// Monotonic time of the last successful sweep, for the health verdict.
    last_ok: Option<Instant>,
    sweep_interval: Duration,
}

/// Shared between the sweep loop and the HTTP handlers.
#[derive(Clone)]
pub struct Status(Arc<RwLock<Inner>>);

/// How many sweep intervals may pass without a successful sweep before `/healthz` fails.
const STALE_AFTER_SWEEPS: u32 = 3;

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Status {
    /// A status for `identity`, sweeping every `sweep_secs`. Unhealthy until the first sweep.
    pub fn new(identity: Identity, sweep_secs: u64) -> Self {
        Self(Arc::new(RwLock::new(Inner {
            identity,
            watching: Vec::new(),
            customers: Vec::new(),
            sweeps: 0,
            errors: 0,
            last_sweep_at: None,
            last_sweep: None,
            last_error: None,
            event_polls: 0,
            event_receipts: 0,
            streams: 0,
            last_ok: None,
            sweep_interval: Duration::from_secs(sweep_secs.max(1)),
        })))
    }

    fn watching_of<S: Store + Clone, K: Signer + Clone>(op: &Operator<S, K>) -> Vec<Watching> {
        op.watching()
            .map(|(chain, dog)| Watching {
                chain: chain.to_string(),
                until: dog.terms().until,
                receipts: dog.receipts().count(),
            })
            .collect()
    }

    /// Record a poll made because a homeserver reported a change.
    pub fn record_event_poll<S: Store + Clone, K: Signer + Clone>(
        &self,
        op: &Operator<S, K>,
        receipts: usize,
    ) {
        let watching = Self::watching_of(op);
        let mut inner = self.0.write().unwrap_or_else(|e| e.into_inner());
        inner.watching = watching;
        inner.event_polls += 1;
        inner.event_receipts += receipts as u64;
    }

    /// Record how many event streams are open.
    pub fn record_streams(&self, streams: usize) {
        let mut inner = self.0.write().unwrap_or_else(|e| e.into_inner());
        inner.streams = streams;
    }

    /// Record a successful sweep: the report, and what the operator is watching and owed.
    pub fn record_sweep<S: Store + Clone, K: Signer + Clone>(
        &self,
        op: &Operator<S, K>,
        customers: &[String],
        report: &SweepReport,
    ) {
        let watching = Self::watching_of(op);
        let customers = customers
            .iter()
            .filter_map(|p| {
                op.credit_of(p).map(|c| Customer {
                    pubky: p.clone(),
                    credit: c.into(),
                })
            })
            .collect();
        let mut inner = self.0.write().unwrap_or_else(|e| e.into_inner());
        inner.watching = watching;
        inner.customers = customers;
        inner.sweeps += 1;
        inner.last_sweep_at = Some(unix_now());
        inner.last_sweep = Some(SweepView::from(report));
        inner.last_error = None;
        inner.last_ok = Some(Instant::now());
    }

    /// Record a failed sweep.
    pub fn record_error(&self, error: &str) {
        let mut inner = self.0.write().unwrap_or_else(|e| e.into_inner());
        inner.sweeps += 1;
        inner.errors += 1;
        inner.last_error = Some(error.to_string());
    }

    /// Whether a sweep succeeded within the last three intervals.
    pub fn healthy(&self) -> bool {
        let inner = self.0.read().unwrap_or_else(|e| e.into_inner());
        inner
            .last_ok
            .map(|t| t.elapsed() <= inner.sweep_interval * STALE_AFTER_SWEEPS)
            .unwrap_or(false)
    }

    /// The `/status` body.
    pub fn snapshot(&self) -> Snapshot {
        let healthy = self.healthy();
        let inner = self.0.read().unwrap_or_else(|e| e.into_inner());
        Snapshot {
            identity: inner.identity.clone(),
            healthy,
            watching: inner.watching.clone(),
            customers: inner.customers.clone(),
            sweeps: inner.sweeps,
            errors: inner.errors,
            last_sweep_at: inner.last_sweep_at,
            last_sweep: inner.last_sweep.clone(),
            last_error: inner.last_error.clone(),
            event_polls: inner.event_polls,
            event_receipts: inner.event_receipts,
            streams: inner.streams,
        }
    }

    /// The routes.
    pub fn router(&self) -> Router {
        Router::new()
            .route("/healthz", get(healthz))
            .route("/status", get(status))
            .with_state(self.clone())
    }

    /// Bind `addr` and serve the routes on a background task.
    pub async fn serve(&self, addr: SocketAddr) -> std::io::Result<tokio::task::JoinHandle<()>> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let router = self.router();
        Ok(tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, router).await {
                tracing::error!(error = %e, "health server stopped");
            }
        }))
    }
}

async fn healthz(State(status): State<Status>) -> (StatusCode, &'static str) {
    if status.healthy() {
        (StatusCode::OK, "ok\n")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "no recent sweep\n")
    }
}

async fn status(State(status): State<Status>) -> Json<Snapshot> {
    Json(status.snapshot())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Identity {
        Identity {
            pubky: "dog".into(),
            kid: "kid".into(),
            client_id: "watchman.example".into(),
            network: "testnet".into(),
            homeserver: "hs".into(),
            path: "/pub/watchman.example/mayfly/".into(),
        }
    }

    #[test]
    fn the_status_json_has_the_documented_shape() {
        let status = Status::new(identity(), 15);
        assert!(!status.healthy(), "nothing has been swept yet");
        let v = serde_json::to_value(status.snapshot()).unwrap();
        assert_eq!(v["pubky"], "dog");
        assert_eq!(v["kid"], "kid");
        assert_eq!(v["client_id"], "watchman.example");
        assert_eq!(v["network"], "testnet");
        assert_eq!(v["homeserver"], "hs");
        assert_eq!(v["path"], "/pub/watchman.example/mayfly/");
        assert_eq!(v["healthy"], false);
        assert_eq!(v["watching"], serde_json::json!([]));
        assert_eq!(v["customers"], serde_json::json!([]));
        assert_eq!(v["sweeps"], 0);
        assert_eq!(v["errors"], 0);
        assert!(v["last_sweep_at"].is_null());
        assert!(v["last_sweep"].is_null());
        assert!(v["last_error"].is_null());
        assert_eq!(v["event_polls"], 0);
        assert_eq!(v["event_receipts"], 0);
        assert_eq!(v["streams"], 0);

        status.record_error("store: boom");
        let v = serde_json::to_value(status.snapshot()).unwrap();
        assert_eq!(v["sweeps"], 1);
        assert_eq!(v["errors"], 1);
        assert_eq!(v["last_error"], "store: boom");
        assert!(!status.healthy());
    }

    #[test]
    fn credit_and_sweep_views_serialise_plainly() {
        assert_eq!(
            serde_json::to_value(CreditView::Free).unwrap(),
            serde_json::json!("free")
        );
        assert_eq!(
            serde_json::to_value(CreditView::Seconds(60)).unwrap(),
            serde_json::json!({ "seconds": 60 })
        );
        let chain = pubky_mayfly::hash::ChainId::derive(b"g");
        let report = SweepReport {
            engaged: vec![chain.clone()],
            declined: vec![(
                "alice".into(),
                chain.clone(),
                pubky_mayfly_watchman::Declined::NotNamed,
            )],
            receipts: 3,
            timed_out: vec![chain.clone()],
            failed: vec![(chain.clone(), "store: away".into())],
            slow_customers: vec!["carol".into()],
            audited: true,
            ..SweepReport::default()
        };
        let v = serde_json::to_value(SweepView::from(&report)).unwrap();
        assert_eq!(v["engaged"], serde_json::json!([chain.to_string()]));
        assert_eq!(v["declined"][0]["why"], "NotNamed");
        assert_eq!(v["declined"][0]["customer"], "alice");
        assert_eq!(v["receipts"], 3);
        assert_eq!(v["timed_out"], serde_json::json!([chain.to_string()]));
        assert_eq!(v["failed"][0]["error"], "store: away");
        assert_eq!(v["slow_customers"], serde_json::json!(["carol"]));
        assert_eq!(v["audited"], true);
    }
}
