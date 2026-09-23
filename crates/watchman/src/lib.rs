//! Mayfly watchman (§11): impartial time and non-rewriting, as a process.
//!
//! A [`Watchman`] runs over the client crate's [`Store`] and [`Signer`]. It publishes an
//! engagement (§11.2), then polls every party's folders and writes **one signed receipt per
//! record** it observes (§11.3) — link, confirmation, reject, or `keys/` revocation — in causal
//! order, with a `consistent` flag; at the `mirror` tier it also keeps byte-for-byte copies
//! (§11.5). It never signs a link, never runs the rules, and nothing in the protocol waits for
//! it: its receipts feed the fold's stopwatch, where every time question is decided by the
//! witness quorum, never by one receipt (§11.2).
//!
//! What it checks before receipting is only what it needs to describe a record honestly: that
//! the bytes decode as a Mayfly record of the type its folder claims, and that the signature
//! verifies under the `kid` the record names, so that `by` in a receipt is never a claim the
//! watchman cannot stand behind. It does not verify Grants, seats or validity — a receipt says
//! "these bytes existed here by this time", nothing more (§11.4).
//!
//! Three rules keep polling order out of the clock (§11.3), and all three are here: a proposal
//! is receipted only after the QC of its `prev` (the embedded copy is receipted first when the
//! confirmers' folders have not been read yet); records at one seq are receipted links first,
//! then votes; and `observed_at` is taken at the moment each receipt is written, so it is
//! monotone within a sweep.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod operator;

pub use operator::{Credit, Declined, Operator, SweepReport};

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::time::Duration;

use pubky_common::auth::grant::GrantClaims;

use pubky_mayfly::genesis::Genesis;
use pubky_mayfly::hash::{ChainId, Hash};
use pubky_mayfly::keys::parse_z32;
use pubky_mayfly::record::{
    Confirmation, Engagement, Kind, Link, Payment, Policy, Receipt, Reject, Revocation, Service,
    Signed, Source,
};
use pubky_mayfly::{typ, PROTOCOL_VERSION};
use pubky_mayfly_client::layout::{Folder, WitnessFolder};
use pubky_mayfly_client::signer::sign_record;
pub use pubky_mayfly_client::{Error, Listed, Signer, Store};

/// What the watchman agrees to (§11.2): the fields of `engage.jws` that are its own to set.
#[derive(Debug, Clone)]
pub struct Terms {
    /// End of engagement, Unix seconds. The watchman stops receipting when it passes.
    pub until: u64,
    /// Polling interval in milliseconds; also the clock tolerance other verifiers allow it.
    pub poll_ms: u64,
    /// Clock source description, e.g. `"ntp"`.
    pub clock: String,
    /// `receipts` or `mirror`.
    pub service: Service,
    /// Proof of payment, if any. The invoice must name the chain (§11.2).
    pub payment: Option<Payment>,
}

impl Terms {
    /// The `receipts` tier, polling every five seconds, until `until`.
    pub fn receipts(until: u64) -> Self {
        Self {
            until,
            poll_ms: 5_000,
            clock: "ntp".into(),
            service: Service::Receipts,
            payment: None,
        }
    }

    /// The `mirror` tier, otherwise as [`Self::receipts`].
    pub fn mirror(until: u64) -> Self {
        Self {
            service: Service::Mirror,
            ..Self::receipts(until)
        }
    }

    /// Set the polling interval.
    pub fn poll_every(mut self, ms: u64) -> Self {
        self.poll_ms = ms;
        self
    }
}

/// What one sweep did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PollReport {
    /// Receipts written.
    pub receipts: usize,
    /// Records receipted with `consistent: false`: a second vote by one key in one round, or a
    /// file whose name claims a hash its bytes do not have (§11.3).
    pub inconsistent: Vec<Hash>,
    /// Byte copies written (`mirror` tier).
    pub mirrored: usize,
    /// `until` has passed: nothing was receipted and nothing will be.
    pub lapsed: bool,
}

/// Where one party's records live: `(owner pubky, protocol folder path)`.
type FolderRef = (String, String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RecordKind {
    Link,
    Confirm,
    Reject,
    Revoke,
}

/// A record found in a party's folder, decoded and signature-checked, not yet receipted.
struct Observed {
    owner: String,
    bytes: Vec<u8>,
    hash: Hash,
    kind: RecordKind,
    typ: &'static str,
    seq: Option<u64>,
    round: Option<u32>,
    by: String,
    /// A link's embedded QC of `prev`, to be receipted before it (§11.3).
    qc: Vec<Vec<u8>>,
    /// The file's name disagreed with its content.
    misnamed: bool,
    /// `(sub-folder, file name)` for the `mirror` tier.
    mirror_at: Option<(String, String)>,
}

/// One receipt this watchman wrote.
#[derive(Debug, Clone)]
pub struct Issued {
    /// The receipt, as written.
    pub receipt: Signed<Receipt>,
    /// The observed record's hash.
    pub record: Hash,
}

/// A watchman for one chain.
pub struct Watchman<S: Store, K: Signer> {
    store: S,
    signer: K,
    chain: ChainId,
    terms: Terms,
    /// Party folders to watch, learned from genesis hints, genesis confirmations and recovers.
    folders: BTreeSet<FolderRef>,
    /// The parties, by pubky, once genesis has been found.
    parties: Vec<String>,
    /// The genesis body, once found.
    genesis: Option<Genesis>,
    /// Files fetched so far: `(owner, path) → hash`, so a sweep fetches only what is new.
    seen: BTreeMap<FolderRef, Hash>,
    /// Records collected by [`Self::engage`]'s discovery pass, receipted at the next poll.
    pending: Vec<Observed>,
    /// Record hash → the receipt issued for it.
    receipted: BTreeMap<Hash, Issued>,
    /// `(kid, seq, round) → first record hash`: one vote per key per round (§6.4).
    votes: BTreeMap<(String, u64, Option<u32>), Hash>,
    engagement: Option<Vec<u8>>,
    cursor: u64,
    clock: Box<dyn Fn() -> u64 + Send + Sync>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Most files fetched per sweep: a hostile folder cannot make the watchman read forever.
const MAX_FILES_PER_SWEEP: usize = 10_000;

impl<S: Store, K: Signer> Watchman<S, K> {
    /// A watchman for `chain`, starting from the folder its genesis was written to (a chain
    /// URL, §9.1). Nothing is read until [`Self::engage`] or [`Self::poll`].
    pub fn new(store: S, signer: K, chain: ChainId, initiator: FolderRef, terms: Terms) -> Self {
        let mut folders = BTreeSet::new();
        folders.insert(initiator);
        Self {
            store,
            signer,
            chain,
            terms,
            folders,
            parties: Vec::new(),
            genesis: None,
            seen: BTreeMap::new(),
            pending: Vec::new(),
            receipted: BTreeMap::new(),
            votes: BTreeMap::new(),
            engagement: None,
            cursor: 0,
            clock: Box::new(now_ms),
        }
    }

    /// Replace the wall clock used for `observed_at` (tests).
    pub fn with_clock(mut self, clock: impl Fn() -> u64 + Send + Sync + 'static) -> Self {
        self.clock = Box::new(clock);
        self
    }

    /// Tell the watchman where else a party writes (an invite, §8.1).
    pub fn add_folder(&mut self, owner: impl Into<String>, path: impl Into<String>) {
        self.folders.insert((owner.into(), path.into()));
    }

    /// The chain.
    pub fn chain(&self) -> &ChainId {
        &self.chain
    }

    /// The signer.
    pub fn signer(&self) -> &K {
        &self.signer
    }

    /// The store.
    pub fn store(&self) -> &S {
        &self.store
    }

    /// The terms, `until` as currently published.
    pub fn terms(&self) -> &Terms {
        &self.terms
    }

    /// Whether `until` has passed on this watchman's clock.
    pub fn is_lapsed(&self) -> bool {
        self.lapsed()
    }

    /// Set `until` before engaging. After engaging, use [`Self::extend`].
    pub fn set_until(&mut self, until: u64) {
        if self.engagement.is_none() {
            self.terms.until = until;
        }
    }

    /// Every folder being watched.
    pub fn folders(&self) -> Vec<FolderRef> {
        self.folders.iter().cloned().collect()
    }

    /// The `engage.jws` bytes, once engaged.
    pub fn engagement(&self) -> Option<&[u8]> {
        self.engagement.as_deref()
    }

    /// Every receipt issued so far, by observed record.
    pub fn receipts(&self) -> impl Iterator<Item = &Issued> {
        self.receipted.values()
    }

    /// When this watchman observed `record`, if it has.
    pub fn observed_at(&self, record: &Hash) -> Option<u64> {
        self.receipted
            .get(record)
            .map(|i| i.receipt.payload.observed_at)
    }

    fn witness_folder(&self) -> WitnessFolder {
        Folder::from_path(&self.signer.path()).witness(&self.chain)
    }

    fn now(&self) -> u64 {
        (self.clock)()
    }

    fn lapsed(&self) -> bool {
        self.now() / 1000 >= self.terms.until
    }

    // ── Engagement (§11.2) ──────────────────────────────────────────────────────────────────

    /// Read the watched folders without receipting anything (what is found is receipted at
    /// the next [`Self::poll`]). Returns whether genesis has been seen.
    pub async fn discover(&mut self) -> Result<bool, Error> {
        let found = self.collect().await?;
        self.pending.extend(found);
        Ok(self.genesis.is_some())
    }

    /// The genesis body, once [`Self::discover`], [`Self::engage`] or [`Self::poll`] has
    /// found it.
    pub fn genesis(&self) -> Option<&Genesis> {
        self.genesis.as_ref()
    }

    /// Publish `engage.jws` (and the historic copy under `engage/<kid>.jws`), naming the
    /// parties genesis names. Reads the initiator's folder to find genesis first.
    pub async fn engage(&mut self) -> Result<Hash, Error> {
        if !self.discover().await? {
            return Err(Error::NoGenesis);
        }
        self.publish().await
    }

    /// Re-engage with a later `until` (§11.2): the later engagement governs going forward.
    /// An engagement is never shortened; that would be equivocation.
    pub async fn extend(&mut self, until: u64) -> Result<Hash, Error> {
        if until <= self.terms.until {
            return Err(Error::State(
                "an engagement is extended, never shortened".into(),
            ));
        }
        if self.engagement.is_none() {
            return Err(Error::State("not engaged".into()));
        }
        self.terms.until = until;
        self.publish().await
    }

    async fn publish(&mut self) -> Result<Hash, Error> {
        let e = Engagement {
            v: PROTOCOL_VERSION,
            kind: "engage".into(),
            chain: self.chain.clone(),
            kid: self.signer.kid(),
            grant: self.signer.grant_jws(),
            path: self.signer.path(),
            parties: self.parties.clone(),
            until: self.terms.until,
            policy: Policy {
                poll_ms: self.terms.poll_ms,
                clock: self.terms.clock.clone(),
            },
            service: self.terms.service,
            payment: self.terms.payment.clone(),
        };
        let bytes = sign_record(&self.signer, typ::WITNESS, &e).await?;
        let wf = self.witness_folder();
        self.store.put(&wf.engagement(), bytes.clone()).await?;
        self.store
            .put(&wf.historic_engagement(&self.signer.kid()), bytes.clone())
            .await?;
        let hash = Hash::of(&bytes);
        self.engagement = Some(bytes);
        Ok(hash)
    }

    // ── Sweeping (§11.3) ────────────────────────────────────────────────────────────────────

    /// One sweep: read every party folder, receipt every new record in causal order, and at
    /// the `mirror` tier copy it. Idempotent: a record is receipted once, by hash.
    pub async fn poll(&mut self) -> Result<PollReport, Error> {
        let mut report = PollReport::default();
        if self.lapsed() {
            report.lapsed = true;
            return Ok(report);
        }
        let mut found = std::mem::take(&mut self.pending);
        found.extend(self.collect().await?);
        // Causal order: by seq, links before votes; revocations (no seq) last; then by hash so
        // two watchmen sweeping the same files write in the same order.
        found.sort_by_key(|o| (o.seq.unwrap_or(u64::MAX), o.kind, o.hash));
        for o in found {
            if self.receipted.contains_key(&o.hash) {
                continue;
            }
            for member in &o.qc {
                let h = Hash::of(member);
                if self.receipted.contains_key(&h) {
                    continue;
                }
                if let Some(m) = self.describe_embedded(&o.owner, member.clone()) {
                    self.issue(&m, &mut report).await?;
                }
            }
            self.issue(&o, &mut report).await?;
        }
        Ok(report)
    }

    /// Poll every `poll_ms` until `stop` resolves or the engagement lapses.
    pub async fn run_until<F: Future<Output = ()>>(&mut self, stop: F) -> Result<(), Error> {
        let mut stop = std::pin::pin!(stop);
        loop {
            if self.poll().await?.lapsed {
                return Ok(());
            }
            tokio::select! {
                () = &mut stop => return Ok(()),
                () = tokio::time::sleep(Duration::from_millis(self.terms.poll_ms)) => {}
            }
        }
    }

    /// Write the receipt for one observation (and its mirror copy).
    async fn issue(&mut self, o: &Observed, report: &mut PollReport) -> Result<(), Error> {
        let mut consistent = !o.misnamed;
        if let Some(seq) = o.seq {
            let key = (o.by.clone(), seq, o.round);
            match self.votes.get(&key) {
                Some(first) if *first != o.hash => consistent = false,
                Some(_) => {}
                None => {
                    self.votes.insert(key, o.hash);
                }
            }
        }
        self.cursor += 1;
        let r = Receipt {
            v: PROTOCOL_VERSION,
            kind: "observed".into(),
            chain: self.chain.clone(),
            record: o.hash.to_base64url(),
            typ: o.typ.to_string(),
            seq: o.seq,
            round: o.round,
            by: o.by.clone(),
            kid: self.signer.kid(),
            observed_at: self.now(),
            source: Source {
                pubky: o.owner.clone(),
                cursor: self.cursor,
            },
            consistent,
        };
        let bytes = sign_record(&self.signer, typ::WITNESS, &r).await?;
        let signed = Signed::<Receipt>::decode(bytes.clone(), typ::WITNESS)?;
        let wf = self.witness_folder();
        let file = match o.seq {
            Some(seq) => wf.receipt(seq, &signed.hash),
            None => wf.revoke_receipt(&o.by, &signed.hash),
        };
        self.store.put(&file, bytes).await?;
        report.receipts += 1;
        if !consistent {
            report.inconsistent.push(o.hash);
        }
        if self.terms.service == Service::Mirror {
            if let Some((sub, name)) = &o.mirror_at {
                self.store
                    .put(&wf.mirror(sub, name), o.bytes.clone())
                    .await?;
                report.mirrored += 1;
            }
        }
        self.receipted.insert(
            o.hash,
            Issued {
                receipt: signed,
                record: o.hash,
            },
        );
        Ok(())
    }

    /// List every watched folder, fetch what is new, decode and signature-check it. Grows the
    /// watched set from what it reads (genesis hints, genesis confirmations, recovers) and
    /// reads again until nothing new appears.
    async fn collect(&mut self) -> Result<Vec<Observed>, Error> {
        let mut out = Vec::new();
        for _ in 0..4 {
            let before = self.folders.len();
            let folders: Vec<FolderRef> = self.folders.iter().cloned().collect();
            let mut listed: Vec<Listed> = Vec::new();
            for (owner, path) in &folders {
                let folder = Folder::from_path(path);
                let prefixes = [
                    folder.chain(&self.chain).as_str().to_string(),
                    format!("{}keys/", folder.as_str()),
                ];
                for prefix in prefixes {
                    listed.extend(self.store.list(owner, &prefix).await?);
                }
            }
            listed.truncate(MAX_FILES_PER_SWEEP);
            for l in listed {
                if !l.path.ends_with(".jws") {
                    continue;
                }
                let key = (l.owner.clone(), l.path.clone());
                // Rejects are named by round and may be overwritten by their author (§7);
                // everything else is named by hash and fetched once.
                let refetch = l.path.contains("/rejects/")
                    || matches!((&l.content_hash, self.seen.get(&key)), (Some(h), Some(s)) if h != s);
                if self.seen.contains_key(&key) && !refetch {
                    continue;
                }
                let Some(bytes) = self.store.get(&l.owner, &l.path).await? else {
                    continue;
                };
                let hash = Hash::of(&bytes);
                if self.seen.get(&key) == Some(&hash) {
                    continue;
                }
                self.seen.insert(key, hash);
                let Some(folder) = folders
                    .iter()
                    .filter(|(o, p)| *o == l.owner && l.path.starts_with(p.as_str()))
                    .map(|(_, p)| p.clone())
                    .max_by_key(String::len)
                else {
                    continue;
                };
                if let Some(o) = self.describe(&l.owner, &l.path[folder.len()..], bytes) {
                    out.push(o);
                }
            }
            if self.folders.len() == before {
                break;
            }
        }
        Ok(out)
    }

    /// Decode a file by where it sits, check its signature under its own `kid`, learn any
    /// folder it declares, and describe it for a receipt. `None` for anything that is not a
    /// record: other witnesses' receipts, mirrors of engagements, undecodable or unsigned bytes.
    fn describe(&mut self, owner: &str, rel: &str, bytes: Vec<u8>) -> Option<Observed> {
        let segments: Vec<&str> = rel.split('/').collect();
        let hash = Hash::of(&bytes);
        match segments.as_slice() {
            ["chains", _, "links", name] => {
                let l = Signed::<Link>::decode(bytes, typ::LINK).ok()?;
                let is_genesis = l.payload.seq == 0
                    && l.payload.prev.is_empty()
                    && ChainId::derive(&l.bytes) == self.chain;
                if !is_genesis && l.payload.chain != self.chain {
                    return None;
                }
                l.verify(&parse_z32(&l.payload.kid).ok()?).ok()?;
                self.learn_from_link(&l, is_genesis);
                let misnamed = name_h16(name, 1) != Some(hash.h16());
                Some(Observed {
                    owner: owner.into(),
                    hash,
                    kind: RecordKind::Link,
                    typ: typ::LINK,
                    seq: Some(l.payload.seq),
                    round: Some(l.payload.round),
                    by: l.payload.kid.clone(),
                    qc: l
                        .payload
                        .confirms
                        .iter()
                        .map(|c| c.as_bytes().to_vec())
                        .collect(),
                    misnamed,
                    mirror_at: Some(("links".into(), (*name).into())),
                    bytes: l.bytes,
                })
            }
            ["chains", _, "confirms", name] => {
                let c = Signed::<Confirmation>::decode(bytes, typ::CONFIRM).ok()?;
                if c.payload.chain != self.chain {
                    return None;
                }
                c.verify(&parse_z32(&c.payload.kid).ok()?).ok()?;
                self.learn_from_confirmation(&c);
                let link = Hash::parse(&c.payload.link).ok()?;
                let misnamed = name_h16(name, 1) != Some(link.h16());
                // A confirmer names its own confirmation by the link alone; a copy in anyone
                // else's folder carries the confirmer's kid too, or two confirmers' copies of
                // one link would collide (§7).
                let mirror_name = mirrored_confirmation_name(&c.payload, &link);
                Some(Observed {
                    owner: owner.into(),
                    hash,
                    kind: RecordKind::Confirm,
                    typ: typ::CONFIRM,
                    seq: Some(c.payload.seq),
                    round: c.payload.round,
                    by: c.payload.kid.clone(),
                    qc: Vec::new(),
                    misnamed,
                    mirror_at: Some(("confirms".into(), mirror_name)),
                    bytes: c.bytes,
                })
            }
            ["chains", _, "rejects", name] => {
                let r = Signed::<Reject>::decode(bytes, typ::REJECT).ok()?;
                if r.payload.chain != self.chain {
                    return None;
                }
                r.verify(&parse_z32(&r.payload.kid).ok()?).ok()?;
                Some(Observed {
                    owner: owner.into(),
                    hash,
                    kind: RecordKind::Reject,
                    typ: typ::REJECT,
                    seq: Some(r.payload.seq),
                    round: Some(r.payload.round),
                    by: r.payload.kid.clone(),
                    qc: Vec::new(),
                    misnamed: false,
                    mirror_at: Some(("rejects".into(), (*name).into())),
                    bytes: r.bytes,
                })
            }
            ["keys", name] if name.ends_with(".revoked.jws") => {
                let r = Signed::<Revocation>::decode(bytes, typ::REVOKE).ok()?;
                r.verify(&parse_z32(&r.payload.by).ok()?).ok()?;
                let misnamed = name.trim_end_matches(".revoked.jws") != r.payload.revoked;
                Some(Observed {
                    owner: owner.into(),
                    hash,
                    kind: RecordKind::Revoke,
                    typ: typ::REVOKE,
                    seq: None,
                    round: None,
                    by: r.payload.revoked.clone(),
                    qc: Vec::new(),
                    misnamed,
                    mirror_at: None,
                    bytes: r.bytes,
                })
            }
            _ => None,
        }
    }

    /// A QC member embedded in `link` that no folder has yielded yet: describe it from the
    /// embedded bytes, observed where the link was.
    fn describe_embedded(&self, owner: &str, bytes: Vec<u8>) -> Option<Observed> {
        let c = Signed::<Confirmation>::decode(bytes, typ::CONFIRM).ok()?;
        if c.payload.chain != self.chain {
            return None;
        }
        c.verify(&parse_z32(&c.payload.kid).ok()?).ok()?;
        let hash = c.hash;
        let link_hash = Hash::parse(&c.payload.link).ok()?;
        let name = mirrored_confirmation_name(&c.payload, &link_hash);
        Some(Observed {
            owner: owner.into(),
            hash,
            kind: RecordKind::Confirm,
            typ: typ::CONFIRM,
            seq: Some(c.payload.seq),
            round: c.payload.round,
            by: c.payload.kid.clone(),
            qc: Vec::new(),
            misnamed: false,
            mirror_at: Some(("confirms".into(), name)),
            bytes: c.bytes,
        })
    }

    /// Genesis names the parties and hints their folders; a `recover` declares a new one.
    fn learn_from_link(&mut self, l: &Signed<Link>, is_genesis: bool) {
        if is_genesis {
            if let Ok(g) = serde_json::from_value::<Genesis>(l.payload.body.clone()) {
                if self.parties.is_empty() {
                    self.parties = g.parties.iter().map(|p| p.pubky.clone()).collect();
                }
                for p in &g.parties {
                    if let Some(path) = &p.path {
                        self.folders.insert((p.pubky.clone(), path.clone()));
                    }
                }
                self.genesis = Some(g);
            }
        } else if l.payload.kind() == Kind::Recover && self.parties.contains(&l.payload.author) {
            if let Some(path) = l.payload.body.get("path").and_then(|p| p.as_str()) {
                self.folders
                    .insert((l.payload.author.clone(), path.to_string()));
            }
        }
    }

    /// A genesis confirmation declares its party's folder (§6.2).
    fn learn_from_confirmation(&mut self, c: &Signed<Confirmation>) {
        if c.payload.seq != 0 {
            return;
        }
        let (Some(grant), Some(path)) = (&c.payload.grant, &c.payload.path) else {
            return;
        };
        let Ok(claims) = GrantClaims::decode(grant) else {
            return;
        };
        let iss = claims.iss.z32();
        if self.parties.contains(&iss) {
            self.folders.insert((iss, path.clone()));
        }
    }
}

/// `<seq8>-<link h16>-<confirmer kid>.jws`: a confirmation as anyone but its author files it.
fn mirrored_confirmation_name(c: &Confirmation, link: &Hash) -> String {
    format!("{:08}-{}-{}.jws", c.seq, link.h16(), c.kid)
}

/// The `<h16>` segment of a `<seq8>-<h16>[-<kid>].jws` name, if it has one.
fn name_h16(name: &str, index: usize) -> Option<String> {
    let stem = name.strip_suffix(".jws")?;
    let part = stem.split('-').nth(index)?;
    (part.len() == 16).then(|| part.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_read_for_their_hash_segment() {
        assert_eq!(
            name_h16("00000007-0123456789ABCDEFG.jws", 1),
            None,
            "seventeen characters is not an h16"
        );
        assert_eq!(
            name_h16("00000007-0123456789ABCDEF.jws", 1).as_deref(),
            Some("0123456789ABCDEF")
        );
        assert_eq!(
            name_h16("00000007-0123456789ABCDEF-somekid.jws", 1).as_deref(),
            Some("0123456789ABCDEF")
        );
        assert_eq!(name_h16("00000007-r1.jws", 1), None);
    }
}
