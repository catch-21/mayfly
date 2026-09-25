//! An operator: one identity watching many chains for its customers (§11.2, "Credit").
//!
//! Nobody asks the operator to watch a chain. A customer's client writes the marker it writes
//! anyway — `index/active/<chain_id>` under its own protocol folder (§7), with the chain URL as
//! its body — and the operator, reading each customer's `/pub/` as its work queue, finds the
//! marker, reads genesis at that URL, and engages if genesis names it and the customer's credit
//! covers it. Payment is settled once, in advance, in watch‑time; each engagement draws on it,
//! and while any remains the operator extends `until` before it lapses. When the marker moves
//! to `index/finished/`, or the credit is gone, the engagement is left to lapse — honestly, by
//! letting `until` pass (§11.2).
//!
//! Some customers are watched for free: an operator's own chains, a demonstration, a test.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;

use pubky_mayfly::hash::ChainId;
use pubky_mayfly_client::layout::Folder;
use pubky_mayfly_client::{Error, Signer, Store};

use crate::{PollReport, Terms, Watchman};

/// How a customer pays (§11.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Credit {
    /// Watched for free, indefinitely.
    Free,
    /// Prepaid watch‑time remaining, in seconds.
    Seconds(u64),
}

/// Why a marker did not lead to an engagement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Declined {
    /// Genesis does not name this operator's pubky as a witness.
    NotNamed,
    /// The customer has no watch‑time left.
    NoCredit,
    /// Genesis could not be read at the marker's URL yet; tried again next sweep.
    NoGenesis,
}

/// What one sweep did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SweepReport {
    /// Chains engaged this sweep.
    pub engaged: Vec<ChainId>,
    /// Chains taken back from an earlier process's engagement and receipts on file, without
    /// a new engagement or any second receipt.
    pub resumed: Vec<ChainId>,
    /// Chains whose `until` was moved later.
    pub extended: Vec<ChainId>,
    /// Chains whose engagement lapsed and are no longer polled.
    pub lapsed: Vec<ChainId>,
    /// Markers seen and not acted on: `(customer, chain, why)`.
    pub declined: Vec<(String, ChainId, Declined)>,
    /// Receipts written across every watched chain.
    pub receipts: usize,
    /// Chains whose poll did not finish within the deadline; tried again next sweep. Their
    /// state is intact: a poll marks nothing done until its receipt is on file.
    pub timed_out: Vec<ChainId>,
    /// Chains whose poll failed, with the error; the other chains were unaffected.
    pub failed: Vec<(ChainId, String)>,
    /// Customers whose `/pub/` could not be listed within the deadline; looked at next sweep.
    pub slow_customers: Vec<String>,
    /// Whether this sweep audited (listed with content hashes) rather than polled by name.
    pub audited: bool,
}

impl SweepReport {
    /// Whether the sweep did anything worth a line in a log.
    pub fn is_quiet(&self) -> bool {
        self.engaged.is_empty()
            && self.resumed.is_empty()
            && self.extended.is_empty()
            && self.lapsed.is_empty()
            && self.declined.is_empty()
            && self.receipts == 0
            && self.timed_out.is_empty()
            && self.failed.is_empty()
            && self.slow_customers.is_empty()
            && !self.audited
    }
}

/// One identity watching many chains.
pub struct Operator<S: Store + Clone, K: Signer + Clone> {
    store: S,
    signer: K,
    terms: Terms,
    engagement_secs: u64,
    renew_before_secs: u64,
    /// How many chains are polled at once.
    concurrency: usize,
    /// How long one chain's poll, one customer's listing, or one engagement may take.
    deadline: Duration,
    /// Every this many sweeps, chains are audited (content hashes) instead of polled.
    audit_every: u32,
    /// Sweeps so far, for the audit cadence.
    sweeps: u32,
    customers: BTreeMap<String, Credit>,
    chains: BTreeMap<ChainId, Watchman<S, K>>,
    /// Which customer each chain is charged to.
    charged_to: BTreeMap<ChainId, String>,
    /// Markers already acted on: `(customer, path)`.
    markers: BTreeSet<(String, String)>,
    /// Chains whose customer has moved the marker to `index/finished/`.
    finished: BTreeSet<ChainId>,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

/// How many chains are polled at once unless [`Operator::concurrency`] says otherwise.
pub const DEFAULT_CONCURRENCY: usize = 8;

/// The deadline for one poll, listing or engagement unless [`Operator::deadline`] says
/// otherwise.
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(15);

/// How many sweeps pass between audits unless [`Operator::audit_every`] says otherwise: at
/// the default fifteen-second sweep, an audit every five minutes.
pub const DEFAULT_AUDIT_EVERY: u32 = 20;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Most `/pub/` entries read per customer per sweep.
const MAX_LISTING: usize = 10_000;

impl<S: Store + Clone, K: Signer + Clone> Operator<S, K> {
    /// An operator publishing under `signer`, engaging on `terms` (whose `until` is ignored:
    /// each engagement runs for [`Self::engage_for`], one day by default, and is renewed).
    pub fn new(store: S, signer: K, terms: Terms) -> Self {
        Self {
            store,
            signer,
            terms,
            engagement_secs: 24 * 3600,
            renew_before_secs: 3600,
            concurrency: DEFAULT_CONCURRENCY,
            deadline: DEFAULT_DEADLINE,
            audit_every: DEFAULT_AUDIT_EVERY,
            sweeps: 0,
            customers: BTreeMap::new(),
            chains: BTreeMap::new(),
            charged_to: BTreeMap::new(),
            markers: BTreeSet::new(),
            finished: BTreeSet::new(),
            clock: Arc::new(now_ms),
        }
    }

    /// Replace the wall clock (tests). Shared with every watchman this operator creates.
    pub fn with_clock(mut self, clock: impl Fn() -> u64 + Send + Sync + 'static) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    /// How long each engagement runs before it is renewed, in seconds.
    pub fn engage_for(mut self, secs: u64) -> Self {
        self.engagement_secs = secs.max(1);
        self
    }

    /// How long before `until` an engagement is renewed, in seconds.
    pub fn renew_before(mut self, secs: u64) -> Self {
        self.renew_before_secs = secs;
        self
    }

    /// How many chains to poll at once (at least one).
    pub fn concurrency(mut self, chains: usize) -> Self {
        self.concurrency = chains.max(1);
        self
    }

    /// How long one chain's poll, one customer's listing or one engagement may take before it
    /// is given up for this sweep. A slow homeserver holds only what is on it.
    pub fn deadline(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }

    /// Every this many sweeps, poll with content hashes ([`Watchman::audit`]) so a mirror
    /// overwritten in place is caught even where no event named it. `0` never audits.
    pub fn audit_every(mut self, sweeps: u32) -> Self {
        self.audit_every = sweeps;
        self
    }

    /// The chain ids being watched.
    pub fn chains(&self) -> impl Iterator<Item = &ChainId> {
        self.chains.keys()
    }

    /// A homeserver said a file under a watched folder was written: read it afresh on the
    /// chain's next poll, even if it has been read before. The chain is what
    /// [`Self::folders_of`] maps the folder to; an unknown chain is ignored.
    pub fn mark_changed(&mut self, chain: &ChainId, owner: &str, path: &str) {
        if let Some(dog) = self.chains.get_mut(chain) {
            dog.note_changed(owner, path);
        }
    }

    /// The folders a watched chain reads: `(owner, protocol folder)` — for whoever wants to
    /// be told when they change rather than poll them.
    pub fn folders_of(&self, chain: &ChainId) -> Vec<(String, String)> {
        self.chains
            .get(chain)
            .map(|dog| dog.folders())
            .unwrap_or_default()
    }

    /// Watch `pubky`'s chains for free.
    pub fn free(&mut self, pubky: impl Into<String>) {
        self.customers.insert(pubky.into(), Credit::Free);
    }

    /// Add `secs` of watch‑time to `pubky`'s credit. A free customer stays free.
    pub fn credit(&mut self, pubky: impl Into<String>, secs: u64) {
        let pubky = pubky.into();
        let next = match self.customers.get(&pubky) {
            Some(Credit::Free) => Credit::Free,
            Some(Credit::Seconds(have)) => Credit::Seconds(have + secs),
            None => Credit::Seconds(secs),
        };
        self.customers.insert(pubky, next);
    }

    /// `pubky`'s credit, if a customer.
    pub fn credit_of(&self, pubky: &str) -> Option<Credit> {
        self.customers.get(pubky).copied()
    }

    /// The watchmen currently engaged, by chain.
    pub fn watching(&self) -> impl Iterator<Item = (&ChainId, &Watchman<S, K>)> {
        self.chains.iter()
    }

    /// The signer.
    pub fn signer(&self) -> &K {
        &self.signer
    }

    fn now_s(&self) -> u64 {
        (self.clock)() / 1000
    }

    /// Take up to `want` seconds from `pubky`'s credit; what was taken (0 if none).
    fn draw(&mut self, pubky: &str, want: u64) -> u64 {
        match self.customers.get_mut(pubky) {
            Some(Credit::Free) => want,
            Some(Credit::Seconds(have)) => {
                let take = want.min(*have);
                *have -= take;
                take
            }
            None => 0,
        }
    }

    /// One sweep: poll every watched chain, a few at a time and each under the deadline; look
    /// for new and moved markers; renew what is near `until` while credit lasts and has not
    /// been marked finished; drop what has lapsed. The engaged chains come first because they
    /// are what the watchman owes time to; discovery and renewal can wait behind them, and a
    /// renewal is due an hour before it is needed.
    pub async fn sweep(&mut self) -> Result<SweepReport, Error> {
        let mut report = SweepReport::default();
        self.sweeps = self.sweeps.wrapping_add(1);
        let thorough = self.audit_every > 0 && self.sweeps.is_multiple_of(self.audit_every);
        report.audited = thorough;
        self.poll_all(thorough, &mut report).await;
        self.read_markers(&mut report).await?;
        self.renew(&mut report).await;
        // What discovery just engaged has its genesis in hand: receipt it now rather than a
        // sweep later. Each under the deadline, as any poll.
        let fresh: Vec<ChainId> = report
            .engaged
            .iter()
            .chain(report.resumed.iter())
            .cloned()
            .collect();
        for chain in fresh {
            match self.poll_chain(&chain).await {
                Ok(Some(poll)) => report.receipts += poll.receipts,
                Ok(None) => {}
                Err(e) if e.to_string().contains("did not finish") => {
                    report.timed_out.push(chain);
                }
                Err(e) => report.failed.push((chain, e.to_string())),
            }
        }
        Ok(report)
    }

    /// Poll one chain now — because its folders changed, say — under the same deadline as a
    /// sweep. `None` if the chain is not watched; `Err` if the poll failed; the report's
    /// `lapsed` if the engagement has passed (the chain is dropped).
    pub async fn poll_chain(&mut self, chain: &ChainId) -> Result<Option<PollReport>, Error> {
        let deadline = self.deadline;
        let Some(dog) = self.chains.get_mut(chain) else {
            return Ok(None);
        };
        let report = match tokio::time::timeout(deadline, dog.poll()).await {
            Ok(r) => r?,
            Err(_) => {
                return Err(Error::Store(format!(
                    "poll of {chain} did not finish within {}s",
                    deadline.as_secs()
                )))
            }
        };
        if report.lapsed {
            self.chains.remove(chain);
        }
        Ok(Some(report))
    }

    /// Every watched chain, `concurrency` at a time, each under the deadline. A chain that
    /// runs out of time or fails is reported and left for the next sweep; the others are
    /// unaffected. `thorough` audits instead of polling.
    async fn poll_all(&mut self, thorough: bool, report: &mut SweepReport) {
        let deadline = self.deadline;
        // Built into a Vec first so no borrow of the map's iterator crosses an await.
        let polls: Vec<_> = self
            .chains
            .iter_mut()
            .map(|(chain, dog)| {
                let chain = chain.clone();
                async move {
                    let outcome = if thorough {
                        tokio::time::timeout(deadline, dog.audit()).await
                    } else {
                        tokio::time::timeout(deadline, dog.poll()).await
                    };
                    (chain, outcome)
                }
            })
            .collect();
        let polls = futures_util::stream::iter(polls)
            .buffer_unordered(self.concurrency)
            .collect::<Vec<_>>()
            .await;
        for (chain, outcome) in polls {
            match outcome {
                Ok(Ok(poll)) => {
                    report.receipts += poll.receipts;
                    if poll.lapsed {
                        self.chains.remove(&chain);
                        report.lapsed.push(chain);
                    }
                }
                Ok(Err(e)) => report.failed.push((chain, e.to_string())),
                Err(_elapsed) => report.timed_out.push(chain),
            }
        }
        report.lapsed.sort();
    }

    /// Extend engagements near `until` while credit lasts. Extensions go to the watchman's own
    /// homeserver; each is still held to the deadline so a slow one cannot stall the sweep.
    async fn renew(&mut self, report: &mut SweepReport) {
        let now_s = self.now_s();
        let renew_before = self.renew_before_secs;
        let engagement_secs = self.engagement_secs;
        let deadline = self.deadline;
        let chains: Vec<ChainId> = self.chains.keys().cloned().collect();
        for chain in chains {
            let Some(dog) = self.chains.get(&chain) else {
                continue;
            };
            let until = dog.terms().until;
            if until.saturating_sub(now_s) > renew_before || self.finished.contains(&chain) {
                continue;
            }
            let Some(customer) = self.charged_to.get(&chain).cloned() else {
                continue;
            };
            let more = self.draw(&customer, engagement_secs);
            if more == 0 {
                continue;
            }
            let Some(dog) = self.chains.get_mut(&chain) else {
                continue;
            };
            match tokio::time::timeout(deadline, dog.extend(until + more)).await {
                Ok(Ok(_)) => report.extended.push(chain),
                Ok(Err(e)) => report.failed.push((chain, e.to_string())),
                Err(_) => report.timed_out.push(chain),
            }
        }
    }

    /// Each customer's `/pub/` is the work queue: an `index/active/<chain_id>` marker under a
    /// protocol folder is a request to watch; `index/finished/` releases it.
    ///
    /// Several parties to one chain may all be customers. One engagement is made and one of
    /// them is charged: a free customer if there is one, else the chain's initiator, else the
    /// first by pubky — and the next in that order if the first has no credit left.
    async fn read_markers(&mut self, report: &mut SweepReport) -> Result<(), Error> {
        let customers: Vec<String> = self.customers.keys().cloned().collect();
        let mut requests: BTreeMap<ChainId, Vec<Marker>> = BTreeMap::new();
        for customer in customers {
            // Names only: a marker is read by fetching it, and a customer's `/pub/` may hold
            // far more than markers. A homeserver that does not answer in time is left for
            // the next sweep; the engaged chains have already been served.
            let listed = {
                let store = self.store.clone();
                let owner = customer.clone();
                let prefix = String::from("/pub/");
                tokio::time::timeout(self.deadline, async move {
                    store.list_names(&owner, &prefix).await
                })
                .await
            };
            let mut listed = match listed {
                Ok(l) => l?,
                Err(_) => {
                    report.slow_customers.push(customer);
                    continue;
                }
            };
            listed.truncate(MAX_LISTING);
            for l in listed {
                let Some((folder, state, id)) = marker(&l.path) else {
                    continue;
                };
                let Ok(chain) = ChainId::parse(id) else {
                    continue;
                };
                if state == "finished" {
                    self.finished.insert(chain);
                    continue;
                }
                let key = (customer.clone(), l.path.clone());
                if self.markers.contains(&key) || self.chains.contains_key(&chain) {
                    continue;
                }
                let body = self
                    .store
                    .get(&customer, &l.path)
                    .await?
                    .and_then(|b| String::from_utf8(b).ok())
                    .unwrap_or_default();
                let initiator = chain_url_folder(body.trim())
                    .unwrap_or_else(|| (customer.clone(), folder.to_string()));
                requests.entry(chain).or_default().push(Marker {
                    customer: customer.clone(),
                    folder: folder.to_string(),
                    path: l.path.clone(),
                    initiator,
                });
            }
        }
        for (chain, mut markers) in requests {
            markers.sort_by_key(|m| {
                (
                    self.customers.get(&m.customer) != Some(&Credit::Free),
                    m.initiator.0 != m.customer,
                    m.customer.clone(),
                )
            });
            for m in markers {
                let key = (m.customer.clone(), m.path.clone());
                self.markers.insert(key.clone());
                let outcome =
                    match tokio::time::timeout(self.deadline, self.engage(&m, &chain)).await {
                        Ok(outcome) => outcome?,
                        Err(_) => {
                            // Genesis or the watchman's own folder did not answer in time; the
                            // marker is looked at again next sweep.
                            self.markers.remove(&key);
                            report.slow_customers.push(m.customer.clone());
                            break;
                        }
                    };
                match outcome {
                    Ok(Engaged::Fresh) => {
                        report.engaged.push(chain.clone());
                        break;
                    }
                    Ok(Engaged::Resumed) => {
                        report.resumed.push(chain.clone());
                        break;
                    }
                    Err(why) => {
                        if why == Declined::NoGenesis {
                            self.markers.remove(&key);
                        }
                        let fatal = why != Declined::NoCredit;
                        report
                            .declined
                            .push((m.customer.clone(), chain.clone(), why));
                        if fatal {
                            break;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Engage on one marker. If this watchman's folder already holds a live engagement for the
    /// chain — an earlier process's — it is taken back as it stands, with every receipt on
    /// file, and no credit is drawn: the customer paid for that time already.
    async fn engage(
        &mut self,
        m: &Marker,
        chain: &ChainId,
    ) -> Result<Result<Engaged, Declined>, Error> {
        let customer = m.customer.as_str();
        let initiator = m.initiator.clone();
        let clock = Arc::clone(&self.clock);
        let mut dog = Watchman::new(
            self.store.clone(),
            self.signer.clone(),
            chain.clone(),
            initiator,
            Terms {
                until: 0,
                ..self.terms.clone()
            },
        )
        .with_clock(move || clock());
        dog.add_folder(customer, m.folder.clone());
        if !dog.discover().await? {
            return Ok(Err(Declined::NoGenesis));
        }
        let me = self.signer.pubky();
        let named = dog
            .genesis()
            .map(|g| g.witnesses.iter().any(|w| w.pubky == me))
            .unwrap_or(false);
        if !named {
            return Ok(Err(Declined::NotNamed));
        }
        let resumed = dog.resume().await?;
        if dog.is_engaged() {
            // The engagement on file is this kid's and still runs: nothing to publish, nothing
            // to charge.
            self.charged_to.insert(chain.clone(), customer.to_string());
            self.chains.insert(chain.clone(), dog);
            return Ok(Ok(Engaged::Resumed));
        }
        // A new engagement, from now for `engagement_secs` — but never ending before the one
        // on file, and charged only for the time beyond it. After a restart with a fresh
        // client key this is how the engagement continues under the new key (§11.2).
        let now = self.now_s();
        let floor = resumed.until.unwrap_or(0).max(now);
        let want = (now + self.engagement_secs).saturating_sub(floor);
        let got = if want == 0 {
            0
        } else {
            self.draw(customer, want)
        };
        if got == 0 && floor == now {
            return Ok(Err(Declined::NoCredit));
        }
        dog.set_until(floor + got);
        dog.engage().await?;
        self.charged_to.insert(chain.clone(), customer.to_string());
        self.chains.insert(chain.clone(), dog);
        Ok(Ok(if resumed.receipts > 0 || resumed.until.is_some() {
            Engaged::Resumed
        } else {
            Engaged::Fresh
        }))
    }
}

/// How a marker led to a watched chain.
enum Engaged {
    /// A new engagement was published and credit drawn.
    Fresh,
    /// An earlier process's engagement was still live and was taken back.
    Resumed,
}

/// One `index/active/<chain_id>` marker as read from a customer's `/pub/`.
struct Marker {
    customer: String,
    /// The protocol folder the marker sits under.
    folder: String,
    path: String,
    /// The chain URL's folder from the marker body, or the customer's own.
    initiator: (String, String),
}

/// `/pub/<app>/mayfly/index/{active,finished}/<chain_id>` → `(protocol folder, state, id)`.
fn marker(path: &str) -> Option<(&str, &str, &str)> {
    let needle = format!("/{}/index/", pubky_mayfly::PROTOCOL_FOLDER);
    let at = path.find(&needle)?;
    let folder = &path[..at + needle.len() - "index/".len()];
    let rest = &path[at + needle.len()..];
    let (state, id) = rest.split_once('/')?;
    if !matches!(state, "active" | "finished") || id.contains('/') {
        return None;
    }
    Some((folder, state, id))
}

/// `pubky://<owner><path>chains/<id>/` → `(owner, path)`.
fn chain_url_folder(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("pubky://")?;
    let slash = rest.find('/')?;
    let owner = &rest[..slash];
    let path = &rest[slash..];
    let at = path.find("chains/")?;
    let folder = &path[..at];
    (!owner.is_empty() && Folder::from_path(folder).as_str() == folder)
        .then(|| (owner.to_string(), folder.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_are_read_from_pub_listings() {
        let id = ChainId::derive(b"g");
        let path = format!("/pub/chess.example/mayfly/index/active/{id}");
        assert_eq!(
            marker(&path),
            Some(("/pub/chess.example/mayfly/", "active", id.as_str()))
        );
        assert_eq!(marker("/pub/chess.example/mayfly/index/other/x"), None);
        assert_eq!(
            marker("/pub/chess.example/mayfly/chains/X/links/00000000-Y.jws"),
            None
        );
    }

    #[test]
    fn chain_urls_name_their_folder() {
        let id = ChainId::derive(b"g");
        let url = format!("pubky://alice/pub/chess.example/mayfly/chains/{id}/");
        assert_eq!(
            chain_url_folder(&url),
            Some(("alice".into(), "/pub/chess.example/mayfly/".into()))
        );
        assert_eq!(chain_url_folder(""), None);
        assert_eq!(chain_url_folder("https://example.com/"), None);
    }
}
