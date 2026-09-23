//! The chain client (§8): sync before voting, then propose / confirm / reject / mirror.
//!
//! One [`ChainClient`] is one party's view of one chain. It never decides anything the fold
//! would not: every action first runs [`fold::verify`] over everything it can read — every
//! folder the chain has declared — and then acts on the [`Verdict`]'s open seq (§9.3). Its
//! own records are written to its own folder and nowhere else; after a commit it mirrors the
//! head and its QC (§7).

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use base64::Engine;
use serde_json::{json, Value};

use pubky_mayfly::fold::{self, Config, Inputs, OpenCandidate, OpenView, Verdict};
use pubky_mayfly::genesis::{self, Genesis};
use pubky_mayfly::hash::{ChainId, Hash};
use pubky_mayfly::record::{
    CloseBody, CloseReason, Confirmation, Engagement, Kind, Link, Receipt, Reject, Signed,
};
use pubky_mayfly::rules::{state_hash, PartyIndex, Rules};
use pubky_mayfly::vote::designated;
use pubky_mayfly::{typ, PROTOCOL_VERSION};

use crate::layout::Folder;
use crate::portable::{Clock, MaybeSend, MaybeSync};
use crate::signer::{sign_record, Signer};
use crate::store::{Listed, Store};
use crate::Error;

/// Client-side policy that is *not* protocol (§11.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// How many engaged witnesses must have receipted the head before this client will confirm
    /// the next link. Default `0`: never holds a vote.
    pub await_witnesses: usize,
    /// Polling interval for [`ChainClient::wait_for_change`] when no event stream is available.
    pub poll: Duration,
    /// Most files fetched per sync (a hostile folder cannot make a client read forever).
    pub max_files_per_sync: usize,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            await_witnesses: 0,
            poll: Duration::from_millis(200),
            max_files_per_sync: 10_000,
        }
    }
}

/// What a party needs to write genesis (§6.6).
#[derive(Debug, Clone)]
pub struct GenesisSpec {
    /// Every party's pubky, the initiator first.
    pub parties: Vec<String>,
    /// Per party, where the initiator expects their records — a discovery hint for their
    /// genesis confirmation (§9.1); `None` where unknown. The initiator's own is filled in.
    pub paths: Vec<Option<String>>,
    /// `confirm_quorum`; `None` for unanimity.
    pub confirm_quorum: Option<u32>,
    /// Witness pubkies.
    pub witnesses: Vec<String>,
    /// `recovery_delay_ms`.
    pub recovery_delay_ms: u64,
    /// `max_body_bytes`.
    pub max_body_bytes: u64,
    /// Rules options.
    pub options: Value,
}

impl GenesisSpec {
    /// Unanimity, no witnesses, defaults, no folder hints.
    pub fn new(parties: Vec<String>) -> Self {
        Self {
            paths: vec![None; parties.len()],
            parties,
            confirm_quorum: None,
            witnesses: Vec::new(),
            recovery_delay_ms: genesis::MIN_RECOVERY_DELAY_MS,
            max_body_bytes: 65_536,
            options: json!({}),
        }
    }

    /// Say which app each party was invited through: `/pub/<client_id>/mayfly/` per party.
    pub fn with_apps(mut self, client_ids: &[&str]) -> Self {
        self.paths = client_ids
            .iter()
            .map(|c| Some(format!("/pub/{c}/{}/", pubky_mayfly::PROTOCOL_FOLDER)))
            .collect();
        self.paths.resize(self.parties.len(), None);
        self
    }
}

/// What a sync produced.
#[derive(Debug, Clone)]
pub struct SyncReport {
    /// The fold's verdict.
    pub verdict: Verdict,
    /// Files whose name does not match their content hash: tampering by the folder owner (§7).
    pub suspects: Vec<Listed>,
    /// Folders that were read.
    pub folders: Vec<(String, String)>,
}

/// Where one party's records live: `(owner pubky, protocol folder path)`.
type FolderRef = (String, String);

/// What [`ChainClient::act`] did, or needs the app to decide (§8.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Confirmed a valid rules proposal in my round.
    Confirmed(Hash),
    /// As the designated proposer of a round after a dead one, re-proposed the lowest-hash
    /// content that received a vote (§6.4).
    Reproposed {
        /// The earlier candidate.
        earlier: Hash,
        /// My new link.
        link: Hash,
    },
    /// As the designated proposer with nothing worth re-proposing, passed (§6.3).
    Passed(Hash),
    /// Skipped a designated proposer silent for `think_ms` on my clock (§6.3, §8.2 step 5).
    Skipped(Hash),
    /// Rejected a link at the open seq in my round that the fold does not admit as a
    /// candidate — invalid under the rules, badly signed, or from an ineligible author — with
    /// the reject as evidence (§8.2 step 4). This spends my vote; the round dies and rotation
    /// moves on.
    Rejected {
        /// The refused link.
        link: Hash,
        /// Why, as far as this client can tell.
        reason: String,
    },
    /// A candidate an honest client never decides alone — a `close`, `recover` or
    /// `witnesses` link (§6.7, §6.8, §11.2) — or a re-proposal of one. Show it to the user;
    /// then [`ChainClient::confirm`], [`ChainClient::reject`], or, if `repropose`,
    /// [`ChainClient::repropose`] or [`ChainClient::pass`].
    Decision {
        /// The candidate.
        candidate: OpenCandidate,
        /// The round I would vote in.
        round: u32,
        /// True when the decision is whether to re-propose it, not whether to confirm it.
        repropose: bool,
    },
    /// Round `round` (≥ 1) is mine to propose in and nothing was voted for in the dead round
    /// before it: propose, or [`ChainClient::pass`].
    MyTurn {
        /// The round.
        round: u32,
    },
    /// Policy (§11.2): holding my vote until the head is witnessed by `want`.
    AwaitingWitnesses {
        /// Receipts held.
        have: usize,
        /// Engaged witnesses.
        of: usize,
        /// Policy.
        want: usize,
    },
}

/// One party's client for one chain.
pub struct ChainClient<R: Rules, S: Store, K: Signer> {
    rules: R,
    store: S,
    signer: K,
    chain: ChainId,
    /// The folder the chain was opened from (the chain URL, §9.1).
    initiator: FolderRef,
    folders: BTreeSet<FolderRef>,
    /// Fetched bytes by `(owner, absolute path)`; files are immutable once named by hash.
    cache: BTreeMap<(String, String), Vec<u8>>,
    inputs: Inputs,
    suspects: Vec<Listed>,
    verdict: Option<Verdict>,
    /// The genesis link, once found: its bytes and body.
    genesis: Option<(Signed<Link>, Genesis)>,
    /// The nonce this party committed to at genesis, when the rules want reveals (§6.6).
    nonce: Option<[u8; 16]>,
    /// A `recover` in flight: the new signer, adopted when the seat's key changes.
    pending_signer: Option<K>,
    /// `(seq, round, when on my clock)` I first saw the round I am in, for skipping (§8.2).
    round_seen: Option<(u64, u32, u64)>,
    /// The seq at which I last skipped: one skip per party per seq (§6.3).
    skipped_at: Option<u64>,
    /// Client policy.
    pub policy: Policy,
    clock: Clock,
}

#[cfg(not(target_arch = "wasm32"))]
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(target_arch = "wasm32")]
fn now_ms() -> u64 {
    // `SystemTime::now` panics on wasm32-unknown-unknown; the browser's clock is `Date.now()`.
    js_sys::Date::now() as u64
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

impl<R: Rules, S: Store, K: Signer> ChainClient<R, S, K> {
    // ── Construction ────────────────────────────────────────────────────────────────────────

    /// A client for an existing chain, starting from the folder its genesis was read from
    /// (a chain URL, §9.1). Nothing is read until [`Self::sync`].
    pub fn open(rules: R, store: S, signer: K, chain: ChainId, initiator: FolderRef) -> Self {
        let mut folders = BTreeSet::new();
        folders.insert(initiator.clone());
        folders.insert((store.me().to_string(), signer.path()));
        Self {
            rules,
            store,
            signer,
            chain,
            initiator,
            folders,
            cache: BTreeMap::new(),
            inputs: Inputs::default(),
            suspects: Vec::new(),
            verdict: None,
            genesis: None,
            nonce: None,
            pending_signer: None,
            round_seen: None,
            skipped_at: None,
            policy: Policy::default(),
            clock: Box::new(now_ms),
        }
    }

    /// A client for the chain an invite names (§8.1): a chain URL
    /// `pubky://<owner>/pub/<client_id>/mayfly/chains/<chain_id>/`. Nothing is read until
    /// [`Self::sync`]; then [`Self::join`] if I am a party.
    pub fn open_url(rules: R, store: S, signer: K, url: &str) -> Result<Self, Error> {
        let (chain, initiator) = crate::layout::parse_chain_url(url)?;
        Ok(Self::open(rules, store, signer, chain, initiator))
    }

    /// The URL to invite others with: this chain at my folder.
    pub fn invite_url(&self) -> String {
        self.chain_url(&(self.store.me().to_string(), self.signer.path()))
    }

    /// Write genesis (§6.6, §8.1) as the initiator and return the client for it.
    pub async fn create(rules: R, store: S, signer: K, spec: GenesisSpec) -> Result<Self, Error> {
        if spec.parties.first() != Some(&signer.pubky()) {
            return Err(Error::State(
                "the initiator is parties[0] and must be the signer".into(),
            ));
        }
        let n = spec.parties.len() as u32;
        let g = Genesis {
            rules: rules.id().to_string(),
            rules_hash: rules.reference_hash().to_string(),
            max_body_bytes: spec.max_body_bytes,
            parties: spec
                .parties
                .iter()
                .enumerate()
                .map(|(i, p)| genesis::Party {
                    pubky: p.clone(),
                    kid: (i == 0).then(|| signer.kid()),
                    role: None,
                    path: if i == 0 {
                        Some(signer.path())
                    } else {
                        spec.paths.get(i).cloned().flatten()
                    },
                })
                .collect(),
            nonce: b64url(&pubky_common::crypto::random_bytes::<16>()),
            confirm_quorum: spec.confirm_quorum.unwrap_or(n),
            witnesses: spec
                .witnesses
                .iter()
                .map(|w| genesis::Witness { pubky: w.clone() })
                .collect(),
            recovery_delay_ms: spec.recovery_delay_ms,
            options: spec.options,
        };
        g.check_safety(
            |id| (id == rules.id()).then(|| rules.reference_hash().to_string()),
            u64::MAX,
        )?;
        let state = rules
            .init(&g, &[], &[])
            .ok()
            .map(|s| state_hash(&rules, &s))
            .unwrap_or_else(|| Hash::of(b"genesis"));
        let ts = now_ms();
        let link = Link {
            v: PROTOCOL_VERSION,
            chain: ChainId::none(),
            seq: 0,
            round: 0,
            prev: String::new(),
            confirms: Vec::new(),
            receipts: Vec::new(),
            author: signer.pubky(),
            kid: signer.kid(),
            ts,
            kind: "genesis".into(),
            body: serde_json::to_value(&g).map_err(|e| Error::State(e.to_string()))?,
            state: state.to_base64url(),
            grant: Some(signer.grant_jws()),
        };
        let bytes = sign_record(&signer, typ::LINK, &link).await?;
        let hash = Hash::of(&bytes);
        let chain = ChainId::derive(&bytes);
        let folder = Folder::from_path(&signer.path());
        let file = folder.chain(&chain).link(0, &hash);
        store.put(&file, bytes.clone()).await?;
        let me = store.me().to_string();
        let mut client = Self::open(
            rules,
            store,
            signer,
            chain,
            (me.clone(), folder.as_str().to_string()),
        );
        client.cache.insert((me, file), bytes);
        client.mark_active().await?;
        Ok(client)
    }

    /// `pubky://<owner><path>chains/<chain>/`: the URL of this chain at `folder` (§9.1).
    pub fn chain_url(&self, folder: &FolderRef) -> String {
        format!(
            "pubky://{}{}",
            folder.0,
            Folder::from_path(&folder.1).chain(&self.chain).as_str()
        )
    }

    /// Write `index/active/<chain>` with the chain URL I joined through as its body (§7): a
    /// UI listing, and the request a credited watchman acts on (§11.2).
    async fn mark_active(&mut self) -> Result<(), Error> {
        let file = self.my_folder().active(&self.chain);
        let body = self.chain_url(&self.initiator).into_bytes();
        self.put_mine(file, body).await?;
        Ok(())
    }

    /// Once the chain is final, move the marker to `index/finished/` (§8.4).
    async fn mark_finished(&mut self) -> Result<(), Error> {
        let me = self.store.me().to_string();
        let active = self.my_folder().active(&self.chain);
        let finished = self.my_folder().finished(&self.chain);
        if self.cache.contains_key(&(me.clone(), finished.clone())) {
            return Ok(());
        }
        let body = self.chain_url(&self.initiator).into_bytes();
        self.put_mine(finished, body).await?;
        if self.cache.remove(&(me, active.clone())).is_some() {
            self.store.delete(&active).await?;
        }
        Ok(())
    }

    /// Replace the wall clock used for `ts` (tests).
    pub fn with_clock(mut self, clock: impl Fn() -> u64 + MaybeSend + MaybeSync + 'static) -> Self {
        self.set_clock(clock);
        self
    }

    /// Replace the wall clock in place: Unix milliseconds. An app with a corrected clock, or
    /// a test, sets it here; skips (§6.3) are judged on it.
    pub fn set_clock(&mut self, clock: impl Fn() -> u64 + MaybeSend + MaybeSync + 'static) {
        self.clock = Box::new(clock);
    }

    /// Tell the client where else to look: a party's folder learned out of band (an invite
    /// link, §8.1). The fold discovers folders from the chain itself once link 1 exists.
    pub fn add_folder(&mut self, owner: impl Into<String>, path: impl Into<String>) {
        self.folders.insert((owner.into(), path.into()));
    }

    /// Every folder this client reads: `(owner, protocol folder path)`.
    pub fn folders(&self) -> Vec<FolderRef> {
        self.folders.iter().cloned().collect()
    }

    /// The chain id.
    pub fn chain(&self) -> &ChainId {
        &self.chain
    }

    /// The signer.
    pub fn signer(&self) -> &K {
        &self.signer
    }

    /// The rules.
    pub fn rules(&self) -> &R {
        &self.rules
    }

    /// The store.
    pub fn store(&self) -> &S {
        &self.store
    }

    /// The last verdict, if any sync has succeeded.
    pub fn verdict(&self) -> Option<&Verdict> {
        self.verdict.as_ref()
    }

    /// My folder.
    fn my_folder(&self) -> Folder {
        Folder::from_path(&self.signer.path())
    }

    // ── Sync (§9.1) ─────────────────────────────────────────────────────────────────────────

    /// Read every known folder, fold, discover the folders the verdict declares, and repeat
    /// until nothing new appears. Returns the report; the verdict is also kept on the client.
    pub async fn sync(&mut self) -> Result<SyncReport, Error> {
        for _ in 0..4 {
            self.read_all().await?;
            let verdict = fold::verify(&self.rules, &self.inputs, &Config::default())?;
            let mut discovered = false;
            // Genesis hints: where the initiator expects each party to write (§9.1).
            if let Some((_, g)) = &self.genesis {
                for p in &g.parties {
                    if let Some(path) = &p.path {
                        discovered |= self.folders.insert((p.pubky.clone(), path.clone()));
                    }
                }
            }
            for seat in &verdict.seats {
                for path in &seat.paths {
                    discovered |= self.folders.insert((seat.pubky.clone(), path.clone()));
                }
            }
            for w in &verdict.engaged {
                discovered |= self.folders.insert((w.pubky.clone(), w.path.clone()));
            }
            // Genesis names a witness by pubky alone; its folder is wherever it published
            // `engage.jws` (§11.2). Until the fold has seen that, look for it in its `/pub/`.
            if let Some((_, g)) = &self.genesis {
                let named: Vec<String> = g
                    .witnesses
                    .iter()
                    .map(|w| w.pubky.clone())
                    .filter(|p| !verdict.engaged.iter().any(|w| w.pubky == *p))
                    .filter(|p| !self.folders.iter().any(|(o, _)| o == p))
                    .collect();
                for pubky in named {
                    for path in find_witness_folders(
                        &self.store,
                        &pubky,
                        &self.chain,
                        self.policy.max_files_per_sync,
                    )
                    .await?
                    {
                        discovered |= self.folders.insert((pubky.clone(), path));
                    }
                }
            }
            // A recover in flight: adopt the new signer once the seat has changed hands.
            if let Some(pending) = &self.pending_signer {
                let me = self.signer.pubky();
                if verdict
                    .seats
                    .iter()
                    .any(|s| s.pubky == me && s.kid == pending.kid())
                {
                    self.signer = self.pending_signer.take().expect("pending");
                    discovered |= self
                        .folders
                        .insert((self.store.me().to_string(), self.signer.path()));
                }
            }
            self.verdict = Some(verdict);
            if !discovered {
                break;
            }
        }
        let is_final = self.verdict.as_ref().is_some_and(Verdict::is_final);
        if is_final && self.my_index().is_ok() {
            self.mark_finished().await?;
        }
        // Remember when the round I am in became current, for skipping.
        if let Some(open) = self.verdict.as_ref().and_then(|v| v.open.as_ref()) {
            let now = (open.seq, Self::current_round(open));
            if self.round_seen.map(|(s, r, _)| (s, r)) != Some(now) {
                self.round_seen = Some((now.0, now.1, self.ts()));
            }
        }
        Ok(SyncReport {
            verdict: self.verdict.clone().expect("set above"),
            suspects: self.suspects.clone(),
            folders: self.folders.iter().cloned().collect(),
        })
    }

    /// List and fetch everything under every known folder into the cache and `inputs`.
    async fn read_all(&mut self) -> Result<(), Error> {
        let folders: Vec<FolderRef> = self.folders.iter().cloned().collect();
        let mut listed_all: Vec<Listed> = Vec::new();
        for (owner, path) in &folders {
            let folder = Folder::from_path(path);
            let prefixes = [
                folder.chain(&self.chain).as_str().to_string(),
                format!("{}keys/", folder.as_str()),
                folder.witness(&self.chain).as_str().to_string(),
            ];
            for prefix in prefixes {
                listed_all.extend(self.store.list(owner, &prefix).await?);
            }
        }
        listed_all.truncate(self.policy.max_files_per_sync);
        for l in listed_all {
            if !l.path.ends_with(".jws") {
                continue;
            }
            let key = (l.owner.clone(), l.path.clone());
            // Rejects are named by round and may be overwritten by their author; everything
            // else is named by hash and immutable — unless the store says the content changed,
            // which is tampering the fold will see (§7).
            let changed = match (&l.content_hash, self.cache.get(&key)) {
                (Some(h), Some(cached)) => *h != Hash::of(cached),
                _ => false,
            };
            let refetch = l.path.contains("/rejects/") || changed;
            if !self.cache.contains_key(&key) || refetch {
                let Some(bytes) = self.store.get(&l.owner, &l.path).await? else {
                    continue;
                };
                self.cache.insert(key.clone(), bytes);
            }
        }
        self.rebuild_inputs(&folders);
        Ok(())
    }

    /// Rebuild `inputs` from the cache: classify by path, check names against hashes.
    fn rebuild_inputs(&mut self, folders: &[FolderRef]) {
        let mut inputs = Inputs {
            chain: Some(self.chain.clone()),
            ..Inputs::default()
        };
        let mut suspects = Vec::new();
        let mut genesis: Option<(Signed<Link>, Genesis)> = None;
        for ((owner, path), bytes) in &self.cache {
            let Some(folder) = folders
                .iter()
                .filter(|(o, p)| o == owner && path.starts_with(p.as_str()))
                .map(|(_, p)| p.clone())
                .max_by_key(String::len)
            else {
                continue;
            };
            let rel = &path[folder.len()..];
            let segments: Vec<&str> = rel.split('/').collect();
            let hash = Hash::of(bytes);
            let name = segments.last().copied().unwrap_or_default();
            let mut check_name = |expected_h16_of_content: bool| {
                if expected_h16_of_content {
                    // `<seq8>-<h16>.jws` or `revoked-<kid>-<h16>.jws`: the h16 is of the file.
                    let stem = name.trim_end_matches(".jws");
                    let h16 = stem.rsplit('-').next().unwrap_or_default();
                    if h16.len() == 16 && h16 != hash.h16() {
                        suspects.push(Listed {
                            owner: owner.clone(),
                            path: path.clone(),
                            content_hash: Some(hash),
                        });
                    }
                }
            };
            let bucket = match segments.as_slice() {
                ["chains", _, "links", _] => {
                    check_name(true);
                    if let Ok(l) = fold::decode_link(bytes.clone()) {
                        if l.payload.seq == 0
                            && l.payload.prev.is_empty()
                            && ChainId::derive(bytes) == self.chain
                        {
                            if let Ok(g) = serde_json::from_value::<Genesis>(l.payload.body.clone())
                            {
                                genesis = Some((l, g));
                            }
                        }
                    }
                    &mut inputs.links
                }
                ["chains", _, "confirms", _] => &mut inputs.confirms,
                ["chains", _, "rejects", _] => &mut inputs.rejects,
                ["chains", _, "receipts", _, "engage.jws"] => &mut inputs.engagements,
                ["chains", _, "receipts", _, _] => {
                    check_name(true);
                    &mut inputs.receipts
                }
                ["witness", _, "engage.jws"] | ["witness", _, "engage", _] => {
                    &mut inputs.engagements
                }
                ["witness", _, _] => {
                    check_name(true);
                    &mut inputs.receipts
                }
                ["keys", file] if file.ends_with(".revoked.jws") => &mut inputs.revocations,
                _ => continue,
            };
            bucket.push(bytes.clone());
            inputs
                .sources
                .entry(hash)
                .or_default()
                .push(format!("pubky://{owner}{folder}"));
        }
        self.inputs = inputs;
        self.suspects = suspects;
        if genesis.is_some() {
            self.genesis = genesis;
        }
    }

    // ── Reading the verdict ─────────────────────────────────────────────────────────────────

    fn require_verdict(&self) -> Result<&Verdict, Error> {
        self.verdict.as_ref().ok_or(Error::NotOpen)
    }

    fn open_view(&self) -> Result<(&Verdict, &OpenView), Error> {
        let v = self.require_verdict()?;
        let open = v.open.as_ref().ok_or(Error::NotOpen)?;
        Ok((v, open))
    }

    /// My party index, once genesis is known.
    pub fn my_index(&self) -> Result<PartyIndex, Error> {
        let (_, g) = self.genesis.as_ref().ok_or(Error::NoGenesis)?;
        let me = self.signer.pubky();
        g.parties
            .iter()
            .position(|p| p.pubky == me)
            .ok_or(Error::NotSeated)
    }

    fn n(&self) -> Result<usize, Error> {
        Ok(self
            .genesis
            .as_ref()
            .ok_or(Error::NoGenesis)?
            .1
            .parties
            .len())
    }

    /// The round I am in at the open seq: the highest round with votes, or the next one if
    /// that round is dead in the files (§6.4).
    fn current_round(open: &OpenView) -> u32 {
        if open.dead {
            open.round + 1
        } else {
            open.round
        }
    }

    fn guard_vote(&self, open: &OpenView, round: u32) -> Result<PartyIndex, Error> {
        let me = self.my_index()?;
        if open
            .voters
            .get(&round)
            .map(|v| v.contains(&me))
            .unwrap_or(false)
        {
            return Err(Error::AlreadyVoted {
                seq: open.seq,
                round,
            });
        }
        Ok(me)
    }

    fn guard_proposer(&self, open: &OpenView, round: u32) -> Result<PartyIndex, Error> {
        let me = self.guard_vote(open, round)?;
        if round >= 1 {
            let d = designated(&self.chain, open.seq, round, self.n()?);
            if d != me {
                return Err(Error::NotDesignated {
                    round,
                    designated: d,
                });
            }
        }
        Ok(me)
    }

    /// The rules state at the head.
    pub fn state(&self) -> Result<Option<R::State>, Error> {
        let v = self.require_verdict()?;
        Ok(fold::replay_state(&self.rules, v)?)
    }

    fn ts(&self) -> u64 {
        (self.clock)()
    }

    // ── Writing records ─────────────────────────────────────────────────────────────────────

    async fn put_mine(&mut self, file: String, bytes: Vec<u8>) -> Result<Hash, Error> {
        let hash = Hash::of(&bytes);
        self.store.put(&file, bytes.clone()).await?;
        self.cache
            .insert((self.store.me().to_string(), file), bytes);
        Ok(hash)
    }

    /// Accept the invitation (§8.1): confirm genesis with my Grant and folder — and, when the
    /// rules want reveals, a commitment to a fresh nonce (§6.6).
    pub async fn join(&mut self) -> Result<Hash, Error> {
        let (g_link, g) = self.genesis.clone().ok_or(Error::NoGenesis)?;
        let me = self.my_index()?;
        if me == 0 {
            return Err(Error::State("the initiator's genesis is their vote".into()));
        }
        let commit = if self.rules.wants_reveals(&g) {
            let nonce = *self
                .nonce
                .get_or_insert_with(pubky_common::crypto::random_bytes::<16>);
            Some(Hash::of(&nonce).to_base64url())
        } else {
            None
        };
        let c = Confirmation {
            v: PROTOCOL_VERSION,
            chain: self.chain.clone(),
            seq: 0,
            round: Some(0),
            link: g_link.hash.to_base64url(),
            kid: self.signer.kid(),
            ts: self.ts(),
            state: g_link.payload.state.clone(),
            grant: Some(self.signer.grant_jws()),
            path: Some(self.signer.path()),
            commit,
        };
        let bytes = sign_record(&self.signer, typ::CONFIRM, &c).await?;
        let file = self.my_folder().chain(&self.chain).confirm(0, &g_link.hash);
        let hash = self.put_mine(file, bytes).await?;
        self.mark_active().await?;
        Ok(hash)
    }

    /// Build a link at the open seq in `round` on the current head.
    fn build_link(
        &self,
        round: u32,
        kind: &str,
        body: Value,
        grant: Option<String>,
    ) -> Result<Link, Error> {
        let (v, open) = self.open_view()?;
        let head = v.head().ok_or(Error::NoGenesis)?;
        let mut confirms: Vec<(String, String)> = head
            .qc
            .iter()
            .map(|c| {
                (
                    c.payload.kid.clone(),
                    String::from_utf8(c.bytes.clone()).expect("jws is ascii"),
                )
            })
            .collect();
        confirms.sort();
        Ok(Link {
            v: PROTOCOL_VERSION,
            chain: self.chain.clone(),
            seq: open.seq,
            round,
            prev: head.link.hash.to_base64url(),
            confirms: confirms.into_iter().map(|(_, c)| c).collect(),
            receipts: self.embeddable_receipts(v),
            author: self.signer.pubky(),
            kid: self.signer.kid(),
            ts: self.ts(),
            kind: kind.into(),
            body,
            state: head.link.payload.state.clone(),
            grant,
        })
    }

    /// Receipts I hold of the head's QC-completing confirmation, one per engaged witness
    /// (§6.1): the receipt with the greatest `observed_at` among the QC's members, ties to the
    /// lower confirmer kid; only if that witness receipted every member.
    fn embeddable_receipts(&self, v: &Verdict) -> Vec<String> {
        let Some(head) = v.head() else {
            return Vec::new();
        };
        let members: BTreeMap<Hash, String> = head
            .qc
            .iter()
            .map(|c| (c.hash, c.payload.kid.clone()))
            .collect();
        let mut by_witness: BTreeMap<String, BTreeMap<Hash, (u64, Vec<u8>)>> = BTreeMap::new();
        for bytes in &self.inputs.receipts {
            let Ok(r) = Signed::<Receipt>::decode(bytes.clone(), typ::WITNESS) else {
                continue;
            };
            let Ok(record) = Hash::parse(&r.payload.record) else {
                continue;
            };
            if members.contains_key(&record) {
                by_witness
                    .entry(r.payload.kid.clone())
                    .or_default()
                    .insert(record, (r.payload.observed_at, bytes.clone()));
            }
        }
        let mut out: Vec<(String, Vec<u8>)> = Vec::new();
        for w in &v.engaged {
            let Some(seen) = by_witness.get(&w.kid) else {
                continue;
            };
            if members.keys().any(|m| !seen.contains_key(m)) {
                continue;
            }
            let best = seen
                .iter()
                .max_by(|(ha, (ta, _)), (hb, (tb, _))| {
                    ta.cmp(tb).then_with(|| members[*hb].cmp(&members[*ha]))
                })
                .map(|(_, (_, b))| b.clone());
            if let Some(b) = best {
                out.push((w.kid.clone(), b));
            }
        }
        out.sort();
        out.into_iter()
            .map(|(_, b)| String::from_utf8(b).expect("jws is ascii"))
            .collect()
    }

    async fn write_link(&mut self, link: Link, signer_is_pending: bool) -> Result<Hash, Error> {
        let bytes = if signer_is_pending {
            let pending = self.pending_signer.as_ref().expect("pending signer");
            sign_record(pending, typ::LINK, &link).await?
        } else {
            sign_record(&self.signer, typ::LINK, &link).await?
        };
        let hash = Hash::of(&bytes);
        let file = self.my_folder().chain(&self.chain).link(link.seq, &hash);
        self.put_mine(file, bytes).await
    }

    /// Propose typed rules content (§8.2): `body` serialises to an object with a `"kind"`
    /// field, which becomes the link's `kind`; the rest is its `body`. This is the shape
    /// `#[serde(tag = "kind")]` gives an enum, and what every rules crate here uses.
    pub async fn propose_body(&mut self, body: &R::Body) -> Result<Hash, Error> {
        let mut value = serde_json::to_value(body).map_err(|e| Error::State(e.to_string()))?;
        let Some(obj) = value.as_object_mut() else {
            return Err(Error::State("a body serialises to an object".into()));
        };
        let kind = match obj.remove("kind") {
            Some(Value::String(k)) => k,
            _ => return Err(Error::State("a body carries its `kind`".into())),
        };
        self.propose(&kind, value).await
    }

    /// Propose rules content of `kind` at the open seq (§8.2).
    pub async fn propose(&mut self, kind: &str, body: Value) -> Result<Hash, Error> {
        let (_, open) = self.open_view()?;
        let round = Self::current_round(open);
        let me = self.guard_proposer(open, round)?;
        let state = self
            .state()?
            .ok_or_else(|| Error::State("the rules have not initialised".into()))?;
        if !self.rules.may_append(&state, me, kind) {
            return Err(Error::Rules(format!("party {me} may not append {kind}")));
        }
        let mut link = self.build_link(round, kind, body, None)?;
        let next = self
            .rules
            .apply(&state, &link)
            .map_err(|e| Error::Rules(e.to_string()))?;
        link.state = state_hash(&self.rules, &next).to_base64url();
        self.write_link(link, false).await
    }

    /// Re-propose an earlier candidate's content in the current round (§6.4: the designated
    /// proposer re-proposes the lowest-hash link that received a vote in the dead round).
    pub async fn repropose(&mut self, earlier: Hash) -> Result<Hash, Error> {
        let bytes = self
            .inputs
            .links
            .iter()
            .find(|b| Hash::of(b) == earlier)
            .cloned()
            .ok_or(Error::NoSuchCandidate)?;
        let link = fold::decode_link(bytes)?;
        match link.payload.kind() {
            pubky_mayfly::record::Kind::Rules => {
                self.propose(&link.payload.kind, link.payload.body).await
            }
            pubky_mayfly::record::Kind::Close => {
                let body: CloseBody = serde_json::from_value(link.payload.body)
                    .map_err(|e| Error::State(e.to_string()))?;
                self.propose_close(body.reason).await
            }
            _ => Err(Error::State(
                "re-proposing a key change is not something an honest client does".into(),
            )),
        }
    }

    /// What the designated proposer re-proposes after a dead round (§6.4): the lowest-hash
    /// link that received a vote in the most recent round that had one — a round killed by
    /// skips has no candidates, so the content carried forward is the last that was proposed.
    pub fn lowest_voted_in_dead_round(&self) -> Result<Option<Hash>, Error> {
        let (_, open) = self.open_view()?;
        if !open.dead {
            return Ok(None);
        }
        let latest = open
            .candidates
            .iter()
            .filter(|c| c.round <= open.round && c.votes > 0)
            .map(|c| c.round)
            .max();
        Ok(latest.and_then(|r| {
            open.candidates
                .iter()
                .filter(|c| c.round == r && c.votes > 0)
                .map(|c| c.hash)
                .min()
        }))
    }

    /// Propose `close {finished | agreed}` (§6.8).
    pub async fn propose_close(&mut self, reason: CloseReason) -> Result<Hash, Error> {
        if reason == CloseReason::Abandoned {
            return Err(Error::State("use propose_abandoned".into()));
        }
        let (_, open) = self.open_view()?;
        let round = Self::current_round(open);
        self.guard_proposer(open, round)?;
        let state = self
            .state()?
            .ok_or_else(|| Error::State("the rules have not initialised".into()))?;
        let body = CloseBody {
            reason,
            subject: Vec::new(),
            pending: Vec::new(),
        };
        self.rules
            .close(&state, &body)
            .map_err(|e| Error::Rules(e.to_string()))?;
        let link = self.build_link(
            round,
            "close",
            serde_json::to_value(&body).map_err(|e| Error::State(e.to_string()))?,
            None,
        )?;
        self.write_link(link, false).await
    }

    /// Propose `close {abandoned}` naming `subjects` (§6.8); outside rounds.
    pub async fn propose_abandoned(&mut self, subjects: &[PartyIndex]) -> Result<Hash, Error> {
        let (_, open) = self.open_view()?;
        let me = self.my_index()?;
        if subjects.contains(&me) || subjects.is_empty() {
            return Err(Error::State(
                "a close names others, and at least one".into(),
            ));
        }
        let g = &self.genesis.as_ref().ok_or(Error::NoGenesis)?.1;
        let body = CloseBody {
            reason: CloseReason::Abandoned,
            subject: subjects
                .iter()
                .map(|s| g.parties[*s].pubky.clone())
                .collect(),
            pending: open
                .candidates
                .iter()
                .map(|c| c.hash.to_base64url())
                .collect(),
        };
        let link = self.build_link(
            0,
            "close",
            serde_json::to_value(&body).map_err(|e| Error::State(e.to_string()))?,
            None,
        )?;
        self.write_link(link, false).await
    }

    /// Confirm an abandoned close: a confirmation with no `round` (§6.8).
    pub async fn confirm_abandoned(&mut self, close: Hash) -> Result<Hash, Error> {
        let (v, open) = self.open_view()?;
        let head = v.head().ok_or(Error::NoGenesis)?;
        let c = Confirmation {
            v: PROTOCOL_VERSION,
            chain: self.chain.clone(),
            seq: open.seq,
            round: None,
            link: close.to_base64url(),
            kid: self.signer.kid(),
            ts: self.ts(),
            state: head.link.payload.state.clone(),
            grant: None,
            path: None,
            commit: None,
        };
        let seq = open.seq;
        let bytes = sign_record(&self.signer, typ::CONFIRM, &c).await?;
        let file = self.my_folder().chain(&self.chain).confirm(seq, &close);
        self.put_mine(file, bytes).await
    }

    /// Reveal my nonce (§6.6).
    pub async fn propose_reveal(&mut self) -> Result<Hash, Error> {
        let nonce = self
            .nonce
            .ok_or_else(|| Error::State("this party committed to no nonce".into()))?;
        let (_, open) = self.open_view()?;
        let round = Self::current_round(open);
        self.guard_proposer(open, round)?;
        let link = self.build_link(round, "reveal", json!({ "nonce": b64url(&nonce) }), None)?;
        self.write_link(link, false).await
    }

    /// `rekey` to `new`: same identity and app, a fresh key and Grant, signed by the current
    /// key (§6.7). The new signer is adopted once the seat has changed hands.
    pub async fn propose_rekey(&mut self, new: K) -> Result<Hash, Error> {
        if new.pubky() != self.signer.pubky() || new.client_id() != self.signer.client_id() {
            return Err(Error::State("rekey keeps the identity and the app".into()));
        }
        let (_, open) = self.open_view()?;
        let round = Self::current_round(open);
        self.guard_proposer(open, round)?;
        let link = self.build_link(
            round,
            "rekey",
            json!({ "new_kid": new.kid() }),
            Some(new.grant_jws()),
        )?;
        self.pending_signer = Some(new);
        self.write_link(link, false).await
    }

    /// `recover` to `new`: same identity, any app, signed by the **new** key (§6.7); the folder
    /// moves to the new app's from the next seq. Adopted once the seat has changed hands.
    pub async fn propose_recover(&mut self, new: K) -> Result<Hash, Error> {
        if new.pubky() != self.signer.pubky() {
            return Err(Error::State("recover keeps the identity".into()));
        }
        let (_, open) = self.open_view()?;
        let round = Self::current_round(open);
        self.guard_proposer(open, round)?;
        let mut link = self.build_link(
            round,
            "recover",
            json!({ "new_kid": new.kid(), "path": new.path() }),
            Some(new.grant_jws()),
        )?;
        link.kid = new.kid();
        self.pending_signer = Some(new);
        self.write_link(link, true).await
    }

    /// Confirm a candidate in the round I am in (§8.2).
    pub async fn confirm(&mut self, link: Hash) -> Result<Hash, Error> {
        let (v, open) = self.open_view()?;
        let round = Self::current_round(open);
        let me = self.guard_vote(open, round)?;
        let cand = open
            .candidates
            .iter()
            .find(|c| c.hash == link)
            .ok_or(Error::NoSuchCandidate)?;
        if cand.round != round {
            return Err(Error::RoundDead);
        }
        if cand.author == me {
            return Err(Error::State(
                "a proposal is already its author's vote".into(),
            ));
        }
        if self.policy.await_witnesses > 0 {
            if let Some(head) = v.head() {
                let (have, of) = head.witnessed;
                if have < self.policy.await_witnesses {
                    return Err(Error::AwaitingWitnesses {
                        have,
                        of,
                        want: self.policy.await_witnesses,
                    });
                }
            }
        }
        let c = Confirmation {
            v: PROTOCOL_VERSION,
            chain: self.chain.clone(),
            seq: open.seq,
            round: Some(round),
            link: link.to_base64url(),
            kid: self.signer.kid(),
            ts: self.ts(),
            state: cand.state.clone(),
            grant: None,
            path: None,
            commit: None,
        };
        let seq = open.seq;
        let bytes = sign_record(&self.signer, typ::CONFIRM, &c).await?;
        let file = self.my_folder().chain(&self.chain).confirm(seq, &link);
        self.put_mine(file, bytes).await
    }

    async fn write_reject(&mut self, round: u32, link: Option<Hash>) -> Result<Hash, Error> {
        let seq = self.open_view()?.1.seq;
        let r = Reject {
            v: PROTOCOL_VERSION,
            chain: self.chain.clone(),
            seq,
            round,
            link: link.map(|h| h.to_base64url()).unwrap_or_default(),
            kid: self.signer.kid(),
            ts: self.ts(),
        };
        let bytes = sign_record(&self.signer, typ::REJECT, &r).await?;
        let file = self.my_folder().chain(&self.chain).reject(seq, round);
        self.put_mine(file, bytes).await
    }

    /// Refuse a candidate in the round I am in (§6.3).
    pub async fn reject(&mut self, link: Hash) -> Result<Hash, Error> {
        let (_, open) = self.open_view()?;
        let round = Self::current_round(open);
        self.guard_vote(open, round)?;
        let cand = open
            .candidates
            .iter()
            .find(|c| c.hash == link)
            .ok_or(Error::NoSuchCandidate)?;
        if cand.round != round {
            return Err(Error::RoundDead);
        }
        self.write_reject(round, Some(link)).await
    }

    /// Pass as the designated proposer (§6.3).
    pub async fn pass(&mut self) -> Result<Hash, Error> {
        let (_, open) = self.open_view()?;
        let round = Self::current_round(open);
        if round == 0 {
            return Err(Error::State("round 0 has no designated proposer".into()));
        }
        self.guard_proposer(open, round)?;
        self.write_reject(round, None).await
    }

    /// Skip a silent designated proposer (§6.3): round `>= 1`, and the fold enforces one per
    /// seq.
    pub async fn skip(&mut self) -> Result<Hash, Error> {
        let (_, open) = self.open_view()?;
        let round = Self::current_round(open);
        if round == 0 {
            return Err(Error::State("no skips in round 0".into()));
        }
        let me = self.guard_vote(open, round)?;
        if designated(&self.chain, open.seq, round, self.n()?) == me {
            return Err(Error::State(
                "the designated proposer passes, not skips".into(),
            ));
        }
        let seq = open.seq;
        let hash = self.write_reject(round, None).await?;
        self.skipped_at = Some(seq);
        Ok(hash)
    }

    // ── The honest step (§8.2) ──────────────────────────────────────────────────────────────

    /// Sync, mirror, and do what an honest party does without asking anyone (§8.2 steps 2–5):
    /// confirm a valid rules proposal in my round; as the designated proposer after a dead
    /// round, re-propose the lowest-hash rules content that received a vote, or pass; skip a
    /// designated proposer who has been silent for the rules' `think_ms` on my clock. What it
    /// will not do alone — vote on a `close`, `recover` or `witnesses` link, or propose — it
    /// returns as [`Action::Decision`] and [`Action::MyTurn`] for the app to put to the user.
    ///
    /// Idempotent: call it whenever anything changes (an event, a poll, a user action). It
    /// never casts a second vote in a round and never skips twice at one seq.
    pub async fn act(&mut self) -> Result<Vec<Action>, Error> {
        self.sync().await?;
        self.mirror().await?;
        let Some(open) = self.verdict.as_ref().and_then(|v| v.open.clone()) else {
            return Ok(Vec::new());
        };
        let me = self.my_index()?;
        let n = self.n()?;
        let round = Self::current_round(&open);
        let voted = open
            .voters
            .get(&round)
            .map(|v| v.contains(&me))
            .unwrap_or(false);
        if voted {
            return Ok(Vec::new());
        }
        let designated_now = round >= 1 && designated(&self.chain, open.seq, round, n) == me;
        let mut out = Vec::new();

        // §8.2 step 4: a link in my round that is not a candidate is refused with evidence.
        if let Some((link, reason)) = self.invalid_in_round(&open, round, me)? {
            self.write_reject(round, Some(link)).await?;
            out.push(Action::Rejected { link, reason });
            return Ok(out);
        }

        if open.dead {
            // The dead round's votes are spent; round `round` has none yet.
            if designated_now {
                match self.lowest_voted_in_dead_round()? {
                    Some(earlier) => {
                        let cand = open.candidates.iter().find(|c| c.hash == earlier).cloned();
                        match cand {
                            Some(c) if Kind::parse(&c.kind) == Kind::Rules => {
                                let link = self.repropose(earlier).await?;
                                out.push(Action::Reproposed { earlier, link });
                            }
                            Some(c) => out.push(Action::Decision {
                                candidate: c,
                                round,
                                repropose: true,
                            }),
                            None => out.push(Action::MyTurn { round }),
                        }
                    }
                    None => out.push(Action::MyTurn { round }),
                }
            } else if let Some(h) = self.maybe_skip(&open, round).await? {
                out.push(Action::Skipped(h));
            }
            return Ok(out);
        }

        let mut live: Vec<OpenCandidate> = open
            .candidates
            .iter()
            .filter(|c| c.round == round && c.author != me)
            .cloned()
            .collect();
        live.sort_by_key(|c| c.hash);
        match live.first() {
            None => {
                if designated_now {
                    out.push(Action::MyTurn { round });
                } else if let Some(h) = self.maybe_skip(&open, round).await? {
                    out.push(Action::Skipped(h));
                }
            }
            Some(c) if Kind::parse(&c.kind) == Kind::Rules => match self.confirm(c.hash).await {
                Ok(_) => out.push(Action::Confirmed(c.hash)),
                Err(Error::AwaitingWitnesses { have, of, want }) => {
                    out.push(Action::AwaitingWitnesses { have, of, want });
                }
                Err(e) => return Err(e),
            },
            Some(c) => out.push(Action::Decision {
                candidate: c.clone(),
                round,
                repropose: false,
            }),
        }
        Ok(out)
    }

    /// The lowest-hash link at the open seq, in `round`, on the head, by another party, that the
    /// fold did not admit as a candidate — and why, as far as the rules can say.
    fn invalid_in_round(
        &self,
        open: &OpenView,
        round: u32,
        me: PartyIndex,
    ) -> Result<Option<(Hash, String)>, Error> {
        let v = self.require_verdict()?;
        let Some(head) = v.head() else {
            return Ok(None);
        };
        let candidates: BTreeSet<Hash> = open.candidates.iter().map(|c| c.hash).collect();
        let kids: BTreeMap<String, PartyIndex> = self
            .genesis
            .iter()
            .flat_map(|(_, g)| g.parties.iter().enumerate())
            .filter_map(|(i, p)| {
                v.seats
                    .iter()
                    .find(|s| s.pubky == p.pubky)
                    .map(|s| (s.kid.clone(), i))
            })
            .collect();
        let state = self.state()?;
        let mut found: Option<(Hash, String)> = None;
        for bytes in &self.inputs.links {
            let hash = Hash::of(bytes);
            if candidates.contains(&hash) || found.as_ref().is_some_and(|(h, _)| *h < hash) {
                continue;
            }
            let Ok(l) = fold::decode_link(bytes.clone()) else {
                continue;
            };
            if l.payload.chain != self.chain
                || l.payload.seq != open.seq
                || l.payload.round != round
                || l.payload.prev_hash().ok().flatten() != Some(head.link.hash)
            {
                continue;
            }
            match kids.get(&l.payload.kid) {
                Some(author) if *author != me => {}
                _ => continue,
            }
            let reason = match (&state, l.payload.kind()) {
                (Some(s), Kind::Rules) => match self.rules.apply(s, &l.payload) {
                    Err(e) => format!("rules: {}", e.0),
                    Ok(_) => "not a candidate (§9.3): signature, eligibility or state".into(),
                },
                _ => "not a candidate (§9.3)".into(),
            };
            found = Some((hash, reason));
        }
        Ok(found)
    }

    /// Skip if the designated proposer of `round` has been silent for `think_ms` on my clock
    /// since the round became current, and I have not skipped at this seq (§8.2 step 5).
    async fn maybe_skip(&mut self, open: &OpenView, round: u32) -> Result<Option<Hash>, Error> {
        if round == 0 || self.skipped_at == Some(open.seq) {
            return Ok(None);
        }
        let Some((seq, r, since)) = self.round_seen else {
            return Ok(None);
        };
        if (seq, r) != (open.seq, round) {
            return Ok(None);
        }
        let think_ms = self
            .genesis
            .as_ref()
            .map(|(_, g)| g.time_control().think_ms)
            .unwrap_or(u64::MAX);
        if self.ts().saturating_sub(since) < think_ms {
            return Ok(None);
        }
        self.skip().await.map(Some)
    }

    // ── Mirroring (§7) ──────────────────────────────────────────────────────────────────────

    /// Mirror every committed link, its QC, the receipts I hold for it and each engaged
    /// witness's `engage.jws` into my folder (§7). Returns how many files were written.
    pub async fn mirror(&mut self) -> Result<usize, Error> {
        let v = self.require_verdict()?.clone();
        let me = self.store.me().to_string();
        let my_kid = self.signer.kid();
        let folder = self.my_folder().chain(&self.chain);
        let mut written = 0;
        let mut writes: Vec<(String, Vec<u8>)> = Vec::new();
        for w in &v.engaged {
            let mine = self.inputs.engagements.iter().find(|bytes| {
                Signed::<Engagement>::decode((*bytes).clone(), typ::WITNESS)
                    .map(|e| e.payload.kid == w.kid && e.payload.until == w.until)
                    .unwrap_or(false)
            });
            if let Some(bytes) = mine {
                writes.push((folder.mirrored_engagement(&w.kid), bytes.clone()));
            }
        }
        for c in &v.committed {
            let seq = c.link.payload.seq;
            writes.push((folder.link(seq, &c.link.hash), c.link.bytes.clone()));
            for q in &c.qc {
                if q.payload.kid != my_kid {
                    writes.push((
                        folder.mirrored_confirm(seq, &c.link.hash, &q.payload.kid),
                        q.bytes.clone(),
                    ));
                }
            }
            let members: BTreeSet<Hash> = c.qc.iter().map(|q| q.hash).collect();
            for bytes in &self.inputs.receipts {
                if let Ok(r) = Signed::<Receipt>::decode(bytes.clone(), typ::WITNESS) {
                    let Ok(record) = Hash::parse(&r.payload.record) else {
                        continue;
                    };
                    if members.contains(&record) {
                        writes.push((
                            folder.mirrored_receipt(&r.payload.kid, seq, &r.hash),
                            bytes.clone(),
                        ));
                    }
                }
            }
        }
        for (file, bytes) in writes {
            if self.cache.contains_key(&(me.clone(), file.clone())) {
                continue;
            }
            self.put_mine(file, bytes).await?;
            written += 1;
        }
        Ok(written)
    }

    // ── Waiting ─────────────────────────────────────────────────────────────────────────────

    /// A cheap fingerprint of every known folder's listing.
    async fn fingerprint(&self) -> Result<BTreeSet<(String, String)>, Error> {
        let mut out = BTreeSet::new();
        for (owner, path) in &self.folders {
            let folder = Folder::from_path(path);
            for l in self
                .store
                .list(owner, folder.chain(&self.chain).as_str())
                .await?
            {
                out.insert((l.owner, l.path));
            }
        }
        Ok(out)
    }

    /// Poll the known folders until their listing changes or `timeout` passes. Returns whether
    /// a change was seen. An app with an event stream (§8.2) wakes on it instead; this is the
    /// fallback every store supports.
    pub async fn wait_for_change(&self, timeout: Duration) -> Result<bool, Error> {
        let before = self.fingerprint().await?;
        // Counted in sleeps rather than read from a monotonic clock, which wasm lacks.
        let mut waited = Duration::ZERO;
        loop {
            crate::time::sleep(self.policy.poll).await;
            waited += self.policy.poll;
            if self.fingerprint().await? != before {
                return Ok(true);
            }
            if waited >= timeout {
                return Ok(false);
            }
        }
    }
}

/// Verify a chain from files alone (§9), as anyone: no seat, no signer. Reads from the folder
/// the chain URL names, then every folder the verdict declares.
pub async fn verify_from<R: Rules, S: Store>(
    rules: &R,
    store: &S,
    chain: &ChainId,
    initiator: FolderRef,
) -> Result<SyncReport, Error> {
    let mut folders: BTreeSet<FolderRef> = BTreeSet::new();
    folders.insert(initiator);
    let mut cache: BTreeMap<(String, String), Vec<u8>> = BTreeMap::new();
    let mut verdict: Option<Verdict> = None;
    let mut suspects = Vec::new();
    for _ in 0..4 {
        let list: Vec<FolderRef> = folders.iter().cloned().collect();
        let mut inputs = Inputs {
            chain: Some(chain.clone()),
            ..Inputs::default()
        };
        suspects.clear();
        for (owner, path) in &list {
            let folder = Folder::from_path(path);
            let prefixes = [
                folder.chain(chain).as_str().to_string(),
                format!("{}keys/", folder.as_str()),
                folder.witness(chain).as_str().to_string(),
            ];
            for prefix in prefixes {
                for l in store.list(owner, &prefix).await? {
                    if !l.path.ends_with(".jws") {
                        continue;
                    }
                    let key = (l.owner.clone(), l.path.clone());
                    if !cache.contains_key(&key) || l.path.contains("/rejects/") {
                        if let Some(bytes) = store.get(&l.owner, &l.path).await? {
                            cache.insert(key.clone(), bytes);
                        }
                    }
                    let Some(bytes) = cache.get(&key) else {
                        continue;
                    };
                    let rel = &l.path[folder.as_str().len()..];
                    let segments: Vec<&str> = rel.split('/').collect();
                    let hash = Hash::of(bytes);
                    let bucket = match segments.as_slice() {
                        ["chains", _, "links", name] => {
                            if name_mismatch(name, &hash) {
                                suspects.push(l.clone());
                            }
                            &mut inputs.links
                        }
                        ["chains", _, "confirms", _] => &mut inputs.confirms,
                        ["chains", _, "rejects", _] => &mut inputs.rejects,
                        ["chains", _, "receipts", _, "engage.jws"] => &mut inputs.engagements,
                        ["chains", _, "receipts", _, _] => &mut inputs.receipts,
                        ["witness", _, "engage.jws"] | ["witness", _, "engage", _] => {
                            &mut inputs.engagements
                        }
                        ["witness", _, _] => &mut inputs.receipts,
                        ["keys", file] if file.ends_with(".revoked.jws") => &mut inputs.revocations,
                        _ => continue,
                    };
                    bucket.push(bytes.clone());
                    inputs
                        .sources
                        .entry(hash)
                        .or_default()
                        .push(format!("pubky://{owner}{path}"));
                }
            }
        }
        let v = fold::verify(rules, &inputs, &Config::default())?;
        let mut discovered = false;
        for l in &inputs.links {
            if let Ok(link) = fold::decode_link(l.clone()) {
                if link.payload.seq == 0 && ChainId::derive(l) == *chain {
                    if let Ok(g) = serde_json::from_value::<Genesis>(link.payload.body.clone()) {
                        for p in &g.parties {
                            if let Some(path) = &p.path {
                                discovered |= folders.insert((p.pubky.clone(), path.clone()));
                            }
                        }
                    }
                }
            }
        }
        for seat in &v.seats {
            for p in &seat.paths {
                discovered |= folders.insert((seat.pubky.clone(), p.clone()));
            }
        }
        for w in &v.engaged {
            discovered |= folders.insert((w.pubky.clone(), w.path.clone()));
        }
        if let Some(g) = v.genesis() {
            for w in &g.witnesses {
                if v.engaged.iter().any(|e| e.pubky == w.pubky)
                    || folders.iter().any(|(o, _)| *o == w.pubky)
                {
                    continue;
                }
                for path in find_witness_folders(
                    store,
                    &w.pubky,
                    chain,
                    Policy::default().max_files_per_sync,
                )
                .await?
                {
                    discovered |= folders.insert((w.pubky.clone(), path));
                }
            }
        }
        verdict = Some(v);
        if !discovered {
            break;
        }
    }
    Ok(SyncReport {
        verdict: verdict.expect("set"),
        suspects,
        folders: folders.into_iter().collect(),
    })
}

/// The protocol folders under `pubky`'s `/pub/` holding a `witness/<chain>/engage.jws`
/// (§11.2): discovery for a witness genesis names by pubky alone. Bounded by `max_files`.
async fn find_witness_folders<S: Store>(
    store: &S,
    pubky: &str,
    chain: &ChainId,
    max_files: usize,
) -> Result<Vec<String>, Error> {
    let suffix = format!("witness/{chain}/engage.jws");
    let mut listed = store.list(pubky, "/pub/").await?;
    listed.truncate(max_files);
    Ok(listed
        .into_iter()
        .filter_map(|l| l.path.strip_suffix(suffix.as_str()).map(str::to_string))
        .filter(|folder| folder.ends_with(&format!("/{}/", pubky_mayfly::PROTOCOL_FOLDER)))
        .collect())
}

/// Does a `<seq8>-<h16>.jws` name disagree with the content's hash?
fn name_mismatch(name: &str, hash: &Hash) -> bool {
    let stem = name.trim_end_matches(".jws");
    let h16 = stem.rsplit('-').next().unwrap_or_default();
    h16.len() == 16 && h16 != hash.h16()
}
