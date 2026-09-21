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

use pubky_mayfly::hash::ChainId;
use pubky_mayfly_client::layout::Folder;
use pubky_mayfly_client::{Error, Signer, Store};

use crate::{Terms, Watchdog};

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
    /// Chains whose `until` was moved later.
    pub extended: Vec<ChainId>,
    /// Chains whose engagement lapsed and are no longer polled.
    pub lapsed: Vec<ChainId>,
    /// Markers seen and not acted on: `(customer, chain, why)`.
    pub declined: Vec<(String, ChainId, Declined)>,
    /// Receipts written across every watched chain.
    pub receipts: usize,
}

/// One identity watching many chains.
pub struct Operator<S: Store + Clone, K: Signer + Clone> {
    store: S,
    signer: K,
    terms: Terms,
    engagement_secs: u64,
    renew_before_secs: u64,
    customers: BTreeMap<String, Credit>,
    chains: BTreeMap<ChainId, Watchdog<S, K>>,
    /// Which customer each chain is charged to.
    charged_to: BTreeMap<ChainId, String>,
    /// Markers already acted on: `(customer, path)`.
    markers: BTreeSet<(String, String)>,
    /// Chains whose customer has moved the marker to `index/finished/`.
    finished: BTreeSet<ChainId>,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

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
            customers: BTreeMap::new(),
            chains: BTreeMap::new(),
            charged_to: BTreeMap::new(),
            markers: BTreeSet::new(),
            finished: BTreeSet::new(),
            clock: Arc::new(now_ms),
        }
    }

    /// Replace the wall clock (tests). Shared with every watchdog this operator creates.
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

    /// The watchdogs currently engaged, by chain.
    pub fn watching(&self) -> impl Iterator<Item = (&ChainId, &Watchdog<S, K>)> {
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

    /// One sweep: find new markers and engage; poll every watched chain; renew what is near
    /// `until` while credit lasts; drop what has lapsed.
    pub async fn sweep(&mut self) -> Result<SweepReport, Error> {
        let mut report = SweepReport::default();
        self.read_markers(&mut report).await?;

        let now_s = self.now_s();
        let renew_before = self.renew_before_secs;
        let engagement_secs = self.engagement_secs;
        let chains: Vec<ChainId> = self.chains.keys().cloned().collect();
        for chain in chains {
            let Some(dog) = self.chains.get_mut(&chain) else {
                continue;
            };
            let poll = dog.poll().await?;
            report.receipts += poll.receipts;
            if poll.lapsed {
                self.chains.remove(&chain);
                report.lapsed.push(chain);
                continue;
            }
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
            if let Some(dog) = self.chains.get_mut(&chain) {
                dog.extend(until + more).await?;
                report.extended.push(chain);
            }
        }
        Ok(report)
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
            let mut listed = self.store.list(&customer, "/pub/").await?;
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
                match self.engage(&m, &chain).await? {
                    Ok(()) => {
                        report.engaged.push(chain.clone());
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

    /// Engage on one marker.
    async fn engage(&mut self, m: &Marker, chain: &ChainId) -> Result<Result<(), Declined>, Error> {
        let customer = m.customer.as_str();
        let initiator = m.initiator.clone();
        let clock = Arc::clone(&self.clock);
        let mut dog = Watchdog::new(
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
        let secs = self.draw(customer, self.engagement_secs);
        if secs == 0 {
            return Ok(Err(Declined::NoCredit));
        }
        let until = self.now_s() + secs;
        dog.set_until(until);
        dog.engage().await?;
        self.charged_to.insert(chain.clone(), customer.to_string());
        self.chains.insert(chain.clone(), dog);
        Ok(Ok(()))
    }
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
