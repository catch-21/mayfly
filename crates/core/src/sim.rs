//! Deterministic N-party simulator for property tests (§16.2 phase 1).
//!
//! The simulator owns keypairs and Grants for `N` parties and `k` witnesses, a fake "storage"
//! per folder, and a model of the chain as an honest client sees it: the committed head with
//! its QC, and the open seq's rounds. Tests drive it with actions — propose, confirm, reject,
//! pass, advance the round — and it produces [`Inputs`] for the fold from any subset of folders,
//! so that a test can ask: does every verifier, given any reachable subset at any moment, agree
//! on everything that is *final*?
//!
//! The model is deliberately independent of [`crate::fold`]: it uses only [`crate::vote`] to
//! decide when a round is dead and when a QC forms, so the fold is checked against a second,
//! simpler reading of §6.4 rather than against itself.
//!
//! Honest actions refuse anything §6.4 discipline forbids and return [`SimError`]. Byzantine
//! behaviour is written with the `forge_*` methods, which write whatever bytes they are asked
//! to and never touch the model.

use std::collections::{BTreeMap, BTreeSet};

use base64::Engine;
use pubky_common::auth::grant::GrantClaims;
use pubky_common::auth::jws::{ClientId, GrantId, GRANT_JWS_TYP};
use pubky_common::capabilities::Capability;
use pubky_common::crypto::Keypair;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::fold::Inputs;
use crate::genesis::{self, Genesis};
use crate::hash::{ChainId, Hash};
use crate::record::{
    sign, CloseBody, CloseReason, Confirmation, Engagement, Link, Policy, Receipt, Reject, Service,
    Signed, Source,
};
use crate::rules::{state_hash, Nonce, Outcome, PartyIndex, Rules, RulesError, Status};
use crate::vote::{designated, effective_quorum, RoundVotes, Vote};
use crate::{typ, PROTOCOL_FOLDER, PROTOCOL_VERSION};

/// SDK default Grant lifetime: two years.
const GRANT_LIFETIME_SECS: u64 = 2 * 365 * 24 * 3600;

/// Something an honest client would never do, or a precondition the test got wrong.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("sim: {0}")]
pub struct SimError(pub String);

fn err<T>(s: impl Into<String>) -> Result<T, SimError> {
    Err(SimError(s.into()))
}

// ─── Test rules ───────────────────────────────────────────────────────────────────────────────

/// `tally/1`: the smallest rules that exercise the protocol. Any party may `add {n}`; anyone
/// may `archive {}`, which is terminal. State is the running total and count.
#[derive(Debug, Default, Clone, Copy)]
pub struct Tally;

/// `tally/1` state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TallyState {
    /// Sum of every `n` added.
    pub total: u64,
    /// Number of adds.
    pub count: u64,
    /// Terminal once archived.
    pub archived: bool,
    /// Number of parties.
    pub parties: usize,
}

/// `tally/1` body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TallyBody {
    /// The number to add (`add` only).
    #[serde(default)]
    pub n: u64,
}

impl Rules for Tally {
    type State = TallyState;
    type Body = TallyBody;

    fn id(&self) -> &'static str {
        "tally/1"
    }

    fn reference_hash(&self) -> &'static str {
        "tally/1-reference-hash"
    }

    fn init(
        &self,
        genesis: &Genesis,
        _confirmations: &[Confirmation],
        _nonces: &[Nonce],
    ) -> Result<TallyState, RulesError> {
        Ok(TallyState {
            parties: genesis.parties.len(),
            ..TallyState::default()
        })
    }

    fn wants_reveals(&self, _genesis: &Genesis) -> bool {
        false
    }

    fn obliged(&self, _state: &TallyState) -> Vec<PartyIndex> {
        Vec::new()
    }

    fn may_append(&self, state: &TallyState, party: PartyIndex, kind: &str) -> bool {
        !state.archived && party < state.parties && matches!(kind, "add" | "archive")
    }

    fn apply(&self, state: &TallyState, link: &Link) -> Result<TallyState, RulesError> {
        if state.archived {
            return Err(RulesError("archived".into()));
        }
        let mut next = state.clone();
        match link.kind.as_str() {
            "add" => {
                let body: TallyBody = serde_json::from_value(link.body.clone())
                    .map_err(|e| RulesError(format!("body: {e}")))?;
                next.total = next
                    .total
                    .checked_add(body.n)
                    .ok_or_else(|| RulesError("overflow".into()))?;
                next.count += 1;
            }
            "archive" => next.archived = true,
            other => return Err(RulesError(format!("unknown kind {other}"))),
        }
        Ok(next)
    }

    fn status(&self, state: &TallyState) -> Status {
        if state.archived {
            Status::Finished(Outcome {
                summary: format!("archived at {}", state.total),
                winners: Vec::new(),
            })
        } else {
            Status::Ongoing
        }
    }

    fn close(&self, state: &TallyState, close: &CloseBody) -> Result<Outcome, RulesError> {
        if close.reason == CloseReason::Finished && !state.archived {
            return Err(RulesError("not archived".into()));
        }
        Ok(Outcome {
            summary: format!("closed at {}", state.total),
            winners: Vec::new(),
        })
    }

    fn canonical_state(&self, state: &TallyState) -> Vec<u8> {
        serde_json::to_vec(state).expect("state serialises")
    }
}

// ─── Actors ───────────────────────────────────────────────────────────────────────────────────

/// One simulated party.
#[derive(Clone)]
pub struct Party {
    /// Identity keypair (would live in Ring).
    pub identity: Keypair,
    /// Chain key: the Grant `cnf`.
    pub client: Keypair,
    /// Grant JWS binding `client` to `identity` with write access to `path`.
    pub grant: String,
    /// The app id.
    pub client_id: String,
    /// `/pub/<client_id>/mayfly/`.
    pub path: String,
    /// Honest or not.
    pub behaviour: Behaviour,
}

impl Party {
    /// Identity, z32.
    pub fn pubky(&self) -> String {
        self.identity.public_key().z32()
    }

    /// Chain key, z32.
    pub fn kid(&self) -> String {
        self.client.public_key().z32()
    }

    /// The storage folder: `pubky://<pubky>/pub/<client_id>/mayfly/`.
    pub fn folder(&self) -> String {
        format!("pubky://{}{}", self.pubky(), self.path)
    }
}

/// How a party behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Behaviour {
    /// Follows §6.4 discipline exactly.
    Honest,
    /// Votes twice in a round when it can.
    Equivocates,
    /// Withholds its confirmation and reveals it after others have moved on.
    WithholdsAndReveals,
    /// Skips every round it is allowed to, immediately.
    SkipsEarly,
    /// Rejects every valid proposal.
    Obstructs,
    /// Deletes its own files after they have been read once.
    DeletesFiles,
    /// Stops responding at a given seq.
    WalksAwayAt(u64),
}

/// One simulated witness.
#[derive(Clone)]
pub struct Witness {
    /// Identity.
    pub identity: Keypair,
    /// Engagement signing key.
    pub client: Keypair,
    /// Grant binding `client` to `identity` with write access to `path`.
    pub grant: String,
    /// The watchdog app's id.
    pub client_id: String,
    /// `/pub/<client_id>/mayfly/`.
    pub path: String,
    /// Its `engage.jws`, once engaged.
    pub engagement: Option<String>,
    /// Behaviour.
    pub behaviour: WitnessBehaviour,
}

impl Witness {
    /// Identity, z32.
    pub fn pubky(&self) -> String {
        self.identity.public_key().z32()
    }

    /// Engagement key, z32.
    pub fn kid(&self) -> String {
        self.client.public_key().z32()
    }

    /// The storage folder.
    pub fn folder(&self) -> String {
        format!("pubky://{}{}", self.pubky(), self.path)
    }
}

/// How a witness behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WitnessBehaviour {
    /// Receipts everything in causal order with a true clock.
    Honest,
    /// Never receipts.
    Dark,
    /// Receipts everything except one party's records.
    Selective(usize),
    /// Receipts with a clock offset in milliseconds.
    Skewed(i64),
    /// Issues receipts, then deletes them from its own storage.
    DeletesReceipts,
}

/// Fake storage: folder → filename → bytes. Deletions remove entries; a party's mirror of
/// another's record is a separate entry under the mirroring party's folder.
#[derive(Debug, Default, Clone)]
pub struct Storage {
    /// Folders.
    pub folders: BTreeMap<String, BTreeMap<String, Vec<u8>>>,
}

impl Storage {
    /// Write (overwriting, as a homeserver `PUT` does).
    pub fn put(&mut self, folder: &str, file: &str, bytes: Vec<u8>) {
        self.folders
            .entry(folder.to_string())
            .or_default()
            .insert(file.to_string(), bytes);
    }

    /// Delete one file.
    pub fn delete(&mut self, folder: &str, file: &str) {
        if let Some(f) = self.folders.get_mut(folder) {
            f.remove(file);
        }
    }

    /// Every `(folder, file, bytes)`.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str, &[u8])> {
        self.folders.iter().flat_map(|(folder, files)| {
            files
                .iter()
                .map(move |(name, bytes)| (folder.as_str(), name.as_str(), bytes.as_slice()))
        })
    }
}

// ─── The model ────────────────────────────────────────────────────────────────────────────────

/// A committed link as the honest model holds it.
#[derive(Clone)]
pub struct Head<S> {
    /// The link's hash.
    pub hash: Hash,
    /// Its bytes.
    pub bytes: Vec<u8>,
    /// Its payload.
    pub link: Link,
    /// Its QC: `(confirmer kid, bytes, hash)`, sorted by kid.
    pub qc: Vec<(String, Vec<u8>, Hash)>,
    /// Rules state after it; `None` before `init`.
    pub state: Option<S>,
}

/// A proposal in the open seq.
#[derive(Clone)]
struct Proposal<S> {
    bytes: Vec<u8>,
    link: Link,
    author: PartyIndex,
    next_state: Option<S>,
    /// `rekey`: the new client key's secret and Grant, applied on commit.
    rekey: Option<([u8; 32], String)>,
}

/// The open seq: rounds, proposals and votes, as an honest client tracks them.
#[derive(Clone)]
pub struct Open<S> {
    /// The open seq.
    pub seq: u64,
    /// The current round.
    pub round: u32,
    /// Votes per round.
    pub rounds: BTreeMap<u32, RoundVotes>,
    proposals: BTreeMap<Hash, Proposal<S>>,
    /// `(link, confirmer) → (kid, bytes, hash)`.
    confirmations: BTreeMap<(Hash, PartyIndex), (String, Vec<u8>, Hash)>,
    skipped: BTreeSet<PartyIndex>,
}

impl<S> Open<S> {
    fn new(seq: u64) -> Self {
        Self {
            seq,
            round: 0,
            rounds: BTreeMap::new(),
            proposals: BTreeMap::new(),
            confirmations: BTreeMap::new(),
            skipped: BTreeSet::new(),
        }
    }

    fn votes(&mut self) -> &mut RoundVotes {
        self.rounds.entry(self.round).or_default()
    }

    fn has_voted(&self, party: PartyIndex) -> bool {
        self.rounds
            .get(&self.round)
            .map(|r| r.by_party.contains_key(&party))
            .unwrap_or(false)
    }

    /// Proposals made in the current round.
    pub fn current_proposals(&self) -> Vec<Hash> {
        let mut v: Vec<Hash> = self
            .proposals
            .iter()
            .filter(|(_, p)| p.link.round == self.round)
            .map(|(h, _)| *h)
            .collect();
        v.sort();
        v
    }
}

/// A simulation in progress.
pub struct Sim<R: Rules> {
    /// The rules every party runs.
    pub rules: R,
    /// Chain id once genesis exists.
    pub chain: Option<ChainId>,
    /// Parties in genesis order.
    pub parties: Vec<Party>,
    /// Witnesses.
    pub witnesses: Vec<Witness>,
    /// All storage.
    pub storage: Storage,
    /// Simulated wall clock, Unix milliseconds.
    pub now_ms: u64,
    /// `confirm_quorum`.
    pub q: usize,
    /// The committed chain.
    pub committed: Vec<Head<R::State>>,
    /// The open seq.
    pub open: Open<R::State>,
    genesis: Option<Genesis>,
    genesis_bytes: Option<Vec<u8>>,
    /// Per witness: `record hash → (observed_at, receipt bytes)`.
    receipted: Vec<BTreeMap<Hash, (u64, Vec<u8>)>>,
    cursor: u64,
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn seq8(seq: u64) -> String {
    format!("{seq:08}")
}

/// Mint a Grant under `identity` for `client` on the app `client_id`.
fn mint_grant(identity: &Keypair, client: &Keypair, client_id: &str, now_s: u64) -> String {
    GrantClaims {
        iss: identity.public_key(),
        client_id: ClientId::new(client_id).expect("client id"),
        caps: vec![Capability::read_write(format!("/pub/{client_id}/")).expect("cap")],
        cnf: client.public_key(),
        jti: GrantId::generate(),
        iat: now_s,
        exp: now_s + GRANT_LIFETIME_SECS,
    }
    .sign(identity, GRANT_JWS_TYP)
}

impl<R: Rules> Sim<R> {
    /// Build `n` parties and `k` witnesses with fresh keys and Grants. `client_ids` lets a test
    /// put parties on different apps (§7); cycled if shorter than `n`. `q` is `confirm_quorum`.
    pub fn new(rules: R, n: usize, k: usize, client_ids: &[&str], q: usize) -> Self {
        let now_ms: u64 = 1_757_779_812_345;
        let now_s = now_ms / 1000;
        let ids: Vec<&str> = if client_ids.is_empty() {
            vec!["chess.example"]
        } else {
            client_ids.to_vec()
        };
        let parties = (0..n)
            .map(|i| {
                let identity = Keypair::random();
                let client = Keypair::random();
                let client_id = ids[i % ids.len()].to_string();
                let grant = mint_grant(&identity, &client, &client_id, now_s);
                Party {
                    identity,
                    client,
                    grant,
                    path: format!("/pub/{client_id}/{PROTOCOL_FOLDER}/"),
                    client_id,
                    behaviour: Behaviour::Honest,
                }
            })
            .collect();
        let witnesses = (0..k)
            .map(|_| {
                let identity = Keypair::random();
                let client = Keypair::random();
                let client_id = "watchdog.example".to_string();
                let grant = mint_grant(&identity, &client, &client_id, now_s);
                Witness {
                    identity,
                    client,
                    grant,
                    path: format!("/pub/{client_id}/{PROTOCOL_FOLDER}/"),
                    client_id,
                    engagement: None,
                    behaviour: WitnessBehaviour::Honest,
                }
            })
            .collect();
        Self {
            rules,
            chain: None,
            parties,
            witnesses,
            storage: Storage::default(),
            now_ms,
            q,
            committed: Vec::new(),
            open: Open::new(0),
            genesis: None,
            genesis_bytes: None,
            receipted: vec![BTreeMap::new(); k],
            cursor: 0,
        }
    }

    // ── Read-side helpers ───────────────────────────────────────────────────────────────────

    /// Number of parties.
    pub fn n(&self) -> usize {
        self.parties.len()
    }

    /// The chain id; panics before genesis.
    pub fn chain_id(&self) -> &ChainId {
        self.chain.as_ref().expect("genesis first")
    }

    /// The committed head.
    pub fn head(&self) -> Option<&Head<R::State>> {
        self.committed.last()
    }

    /// Hashes of the committed links, seq 0 upward.
    pub fn committed_hashes(&self) -> Vec<Hash> {
        self.committed.iter().map(|h| h.hash).collect()
    }

    /// The designated proposer of the current round, if `round >= 1`.
    pub fn designated(&self) -> Option<PartyIndex> {
        (self.open.round >= 1)
            .then(|| designated(self.chain_id(), self.open.seq, self.open.round, self.n()))
    }

    /// Is the current round dead in the model?
    pub fn round_is_dead(&self) -> bool {
        self.open
            .rounds
            .get(&self.open.round)
            .map(|r| r.is_dead(self.n(), self.q))
            .unwrap_or(false)
    }

    /// The lowest-hash link that received any vote in the previous round (§6.4).
    pub fn lowest_voted_in_previous_round(&self) -> Option<Hash> {
        let r = self.open.round.checked_sub(1)?;
        self.open.rounds.get(&r)?.lowest_voted()
    }

    /// Advance the clock.
    pub fn tick(&mut self, ms: u64) {
        self.now_ms += ms;
    }

    fn chain_folder(&self) -> String {
        format!("chains/{}/", self.chain_id())
    }

    fn write_party(&mut self, party: PartyIndex, file: &str, bytes: Vec<u8>) {
        let folder = self.parties[party].folder();
        self.storage.put(&folder, file, bytes);
    }

    // ── Genesis ─────────────────────────────────────────────────────────────────────────────

    /// The initiator (party 0) writes genesis. Returns its hash.
    pub fn genesis(&mut self) -> Result<Hash, SimError> {
        if self.chain.is_some() {
            return err("genesis already written");
        }
        let n = self.n();
        if self.q == 0 || self.q > n || 2 * self.q <= n {
            return err(format!("q={} not in N/2 < q <= N", self.q));
        }
        let g = Genesis {
            rules: self.rules.id().to_string(),
            rules_hash: self.rules.reference_hash().to_string(),
            max_body_bytes: 65_536,
            parties: self
                .parties
                .iter()
                .enumerate()
                .map(|(i, p)| genesis::Party {
                    pubky: p.pubky(),
                    kid: (i == 0).then(|| p.kid()),
                    role: None,
                })
                .collect(),
            nonce: b64url(&pubky_common::crypto::random_bytes::<16>()),
            confirm_quorum: self.q as u32,
            witnesses: self
                .witnesses
                .iter()
                .map(|w| genesis::Witness { pubky: w.pubky() })
                .collect(),
            recovery_delay_ms: genesis::MIN_RECOVERY_DELAY_MS,
            options: json!({}),
        };
        let state = self
            .rules
            .init(&g, &[], &[])
            .ok()
            .map(|s| state_hash(&self.rules, &s))
            .unwrap_or_else(|| Hash::of(b"genesis"));
        let link = Link {
            v: PROTOCOL_VERSION,
            chain: ChainId::none(),
            seq: 0,
            round: 0,
            prev: String::new(),
            confirms: Vec::new(),
            receipts: Vec::new(),
            author: self.parties[0].pubky(),
            kid: self.parties[0].kid(),
            ts: self.now_ms,
            kind: "genesis".into(),
            body: serde_json::to_value(&g).expect("genesis serialises"),
            state: state.to_base64url(),
            grant: Some(self.parties[0].grant.clone()),
        };
        let bytes =
            sign(&self.parties[0].client, typ::LINK, &link).map_err(|e| SimError(e.to_string()))?;
        let hash = Hash::of(&bytes);
        self.chain = Some(ChainId::derive(&bytes));
        self.genesis = Some(g);
        self.genesis_bytes = Some(bytes.clone());
        let file = format!(
            "{}links/{}-{}.jws",
            self.chain_folder(),
            seq8(0),
            hash.h16()
        );
        self.write_party(0, &file, bytes.clone());
        self.open = Open::new(0);
        self.open.votes().propose(0, hash);
        self.open.proposals.insert(
            hash,
            Proposal {
                bytes,
                link,
                author: 0,
                next_state: None,
                rekey: None,
            },
        );
        Ok(hash)
    }

    /// Party `p` accepts the invitation: a genesis confirmation with Grant and path.
    ///
    /// Under `q < N` genesis may already have committed without `p`; the confirmation is then
    /// no longer a vote but is still how `p`'s key and folder become known (§5.3, §6.6), so an
    /// honest client publishes it anyway.
    pub fn confirm_genesis(&mut self, p: PartyIndex) -> Result<(), SimError> {
        if self.chain.is_none() {
            return err("no genesis yet");
        }
        if p == 0 {
            return err("the initiator's proposal is their vote");
        }
        let (hash, state) = match self.committed.first() {
            Some(g) => (g.hash, g.link.state.clone()),
            None => {
                let (h, prop) = self.open.proposals.iter().next().expect("genesis proposal");
                (*h, prop.link.state.clone())
            }
        };
        let party = &self.parties[p];
        let c = Confirmation {
            v: PROTOCOL_VERSION,
            chain: self.chain_id().clone(),
            seq: 0,
            round: Some(0),
            link: hash.to_base64url(),
            kid: party.kid(),
            ts: self.now_ms,
            state,
            grant: Some(party.grant.clone()),
            path: Some(party.path.clone()),
            commit: None,
        };
        if self.committed.is_empty() {
            self.write_confirmation(p, hash, &c)?;
            self.try_commit();
        } else {
            // Late: the only evidence of this seat until a link embeds it, so every party
            // mirrors it as they would a QC (§7).
            let bytes = sign(&self.parties[p].client, typ::CONFIRM, &c)
                .map_err(|e| SimError(e.to_string()))?;
            let kid = c.kid.clone();
            let chain_folder = self.chain_folder();
            self.write_party(
                p,
                &format!("{chain_folder}confirms/{}-{}.jws", seq8(0), hash.h16()),
                bytes.clone(),
            );
            for other in 0..self.n() {
                if other != p {
                    self.write_party(
                        other,
                        &format!(
                            "{chain_folder}confirms/{}-{}-{kid}.jws",
                            seq8(0),
                            hash.h16()
                        ),
                        bytes.clone(),
                    );
                }
            }
        }
        Ok(())
    }

    /// Every witness named at genesis publishes its `engage.jws`.
    pub fn engage_witnesses(&mut self) -> Result<(), SimError> {
        let chain = self.chain_id().clone();
        let parties: Vec<String> = self.parties.iter().map(Party::pubky).collect();
        for i in 0..self.witnesses.len() {
            let w = &self.witnesses[i];
            let e = Engagement {
                v: PROTOCOL_VERSION,
                kind: "engage".into(),
                chain: chain.clone(),
                kid: w.kid(),
                grant: w.grant.clone(),
                path: w.path.clone(),
                parties: parties.clone(),
                until: self.now_ms / 1000 + 365 * 24 * 3600,
                policy: Policy {
                    poll_ms: 1000,
                    clock: "sim".into(),
                },
                service: Service::Receipts,
                payment: None,
            };
            let bytes = sign(&w.client, typ::WITNESS, &e).map_err(|e| SimError(e.to_string()))?;
            let text = String::from_utf8(bytes.clone()).expect("jws is ascii");
            let folder = w.folder();
            let file = format!("witness/{chain}/engage.jws");
            self.storage.put(&folder, &file, bytes);
            self.witnesses[i].engagement = Some(text);
        }
        Ok(())
    }

    // ── Honest actions ──────────────────────────────────────────────────────────────────────

    fn guard_vote(&self, p: PartyIndex) -> Result<(), SimError> {
        if self.committed.is_empty() {
            return err("genesis has not committed");
        }
        if p >= self.n() {
            return err("no such party");
        }
        if self.open.has_voted(p) {
            return err(format!(
                "party {p} already voted in round {} of seq {}",
                self.open.round, self.open.seq
            ));
        }
        Ok(())
    }

    fn guard_proposer(&self, p: PartyIndex) -> Result<(), SimError> {
        self.guard_vote(p)?;
        if let Some(d) = self.designated() {
            if d != p {
                return err(format!("round {} is {d}'s to propose", self.open.round));
            }
        }
        Ok(())
    }

    /// Receipts of the head's QC this proposer holds, one per witness: the receipt of the
    /// QC-completing confirmation as that witness saw it (§6.1), sorted by witness kid.
    fn embeddable_receipts(&self) -> Vec<String> {
        let Some(head) = self.head() else {
            return Vec::new();
        };
        let mut out: Vec<(String, String)> = Vec::new();
        for (i, w) in self.witnesses.iter().enumerate() {
            let seen = &self.receipted[i];
            let mut best: Option<(u64, String, Vec<u8>)> = None;
            let mut complete = true;
            for (kid, _, h) in &head.qc {
                match seen.get(h) {
                    Some((at, bytes)) => {
                        // Greatest observed_at; ties to the lower kid (§6.1).
                        let better = match &best {
                            None => true,
                            Some((a, k, _)) => *at > *a || (*at == *a && kid < k),
                        };
                        if better {
                            best = Some((*at, kid.clone(), bytes.clone()));
                        }
                    }
                    None => complete = false,
                }
            }
            if let (true, Some((_, _, bytes))) = (complete, best) {
                out.push((w.kid(), String::from_utf8(bytes).expect("ascii")));
            }
        }
        out.sort();
        out.into_iter().map(|(_, r)| r).collect()
    }

    fn build_link(
        &self,
        p: PartyIndex,
        round: u32,
        kind: &str,
        body: Value,
        grant: Option<String>,
    ) -> Link {
        let head = self.head().expect("guarded");
        Link {
            v: PROTOCOL_VERSION,
            chain: self.chain_id().clone(),
            seq: self.open.seq,
            round,
            prev: head.hash.to_base64url(),
            confirms: head
                .qc
                .iter()
                .map(|(_, b, _)| String::from_utf8(b.clone()).expect("ascii"))
                .collect(),
            receipts: self.embeddable_receipts(),
            author: self.parties[p].pubky(),
            kid: self.parties[p].kid(),
            ts: self.now_ms,
            kind: kind.into(),
            body,
            state: head.link.state.clone(),
            grant,
        }
    }

    fn write_proposal(
        &mut self,
        p: PartyIndex,
        link: Link,
        next_state: Option<R::State>,
        rekey: Option<([u8; 32], String)>,
    ) -> Result<Hash, SimError> {
        let bytes =
            sign(&self.parties[p].client, typ::LINK, &link).map_err(|e| SimError(e.to_string()))?;
        let hash = Hash::of(&bytes);
        let file = format!(
            "{}links/{}-{}.jws",
            self.chain_folder(),
            seq8(link.seq),
            hash.h16()
        );
        self.write_party(p, &file, bytes.clone());
        self.open.votes().propose(p, hash);
        self.open.proposals.insert(
            hash,
            Proposal {
                bytes,
                link,
                author: p,
                next_state,
                rekey,
            },
        );
        Ok(hash)
    }

    /// `p` proposes rules content of `kind` in the current round.
    pub fn propose(&mut self, p: PartyIndex, kind: &str, body: Value) -> Result<Hash, SimError> {
        self.guard_proposer(p)?;
        let state = self
            .head()
            .and_then(|h| h.state.clone())
            .ok_or_else(|| SimError("rules state not initialised".into()))?;
        if !self.rules.may_append(&state, p, kind) {
            return err(format!("party {p} may not append {kind}"));
        }
        let mut link = self.build_link(p, self.open.round, kind, body, None);
        let next = self
            .rules
            .apply(&state, &link)
            .map_err(|e| SimError(format!("rules: {e}")))?;
        link.state = state_hash(&self.rules, &next).to_base64url();
        self.write_proposal(p, link, Some(next), None)
    }

    /// `p` re-proposes the content of an earlier proposal at this seq in the current round
    /// (§6.4: the designated proposer re-proposes the lowest-hash link that received a vote).
    pub fn repropose(&mut self, p: PartyIndex, earlier: Hash) -> Result<Hash, SimError> {
        let prop = self
            .open
            .proposals
            .get(&earlier)
            .cloned()
            .ok_or_else(|| SimError("no such proposal at this seq".into()))?;
        match prop.link.kind() {
            crate::record::Kind::Rules => self.propose(p, &prop.link.kind, prop.link.body),
            crate::record::Kind::Close => {
                let body: CloseBody = serde_json::from_value(prop.link.body).expect("close body");
                self.propose_close(p, body.reason)
            }
            _ => err("re-proposing a key change is not something an honest client does"),
        }
    }

    /// `p` proposes `close {finished | agreed}`.
    pub fn propose_close(&mut self, p: PartyIndex, reason: CloseReason) -> Result<Hash, SimError> {
        self.guard_proposer(p)?;
        if reason == CloseReason::Abandoned {
            return err("use propose_abandoned");
        }
        let state = self
            .head()
            .and_then(|h| h.state.clone())
            .ok_or_else(|| SimError("rules state not initialised".into()))?;
        let body = CloseBody {
            reason,
            subject: Vec::new(),
            pending: Vec::new(),
        };
        self.rules
            .close(&state, &body)
            .map_err(|e| SimError(format!("rules: {e}")))?;
        let link = self.build_link(
            p,
            self.open.round,
            "close",
            serde_json::to_value(&body).expect("close body"),
            None,
        );
        self.write_proposal(p, link, Some(state), None)
    }

    /// `p` proposes `rekey` to a fresh client key under the same app.
    pub fn propose_rekey(&mut self, p: PartyIndex) -> Result<Hash, SimError> {
        self.guard_proposer(p)?;
        let new_client = Keypair::random();
        let party = &self.parties[p];
        let grant = mint_grant(
            &party.identity,
            &new_client,
            &party.client_id,
            self.now_ms / 1000,
        );
        let body = json!({ "new_kid": new_client.public_key().z32() });
        let link = self.build_link(p, self.open.round, "rekey", body, Some(grant.clone()));
        let state = self.head().and_then(|h| h.state.clone());
        self.write_proposal(p, link, state, Some((new_client.secret(), grant)))
    }

    fn write_confirmation(
        &mut self,
        p: PartyIndex,
        link: Hash,
        c: &Confirmation,
    ) -> Result<Hash, SimError> {
        let bytes =
            sign(&self.parties[p].client, typ::CONFIRM, c).map_err(|e| SimError(e.to_string()))?;
        let hash = Hash::of(&bytes);
        let file = format!(
            "{}confirms/{}-{}.jws",
            self.chain_folder(),
            seq8(c.seq),
            link.h16()
        );
        self.write_party(p, &file, bytes.clone());
        self.open.votes().cast(p, Vote::For(link));
        self.open
            .confirmations
            .insert((link, p), (c.kid.clone(), bytes, hash));
        Ok(hash)
    }

    /// `p` confirms a proposal made in the current round.
    pub fn confirm(&mut self, p: PartyIndex, link: Hash) -> Result<(), SimError> {
        self.guard_vote(p)?;
        let prop = self
            .open
            .proposals
            .get(&link)
            .cloned()
            .ok_or_else(|| SimError("no such proposal".into()))?;
        if prop.link.round != self.open.round {
            return err("that proposal is in another round");
        }
        if prop.author == p {
            return err("a proposal is already its author's vote");
        }
        let c = Confirmation {
            v: PROTOCOL_VERSION,
            chain: self.chain_id().clone(),
            seq: self.open.seq,
            round: Some(self.open.round),
            link: link.to_base64url(),
            kid: self.parties[p].kid(),
            ts: self.now_ms,
            state: prop.link.state.clone(),
            grant: None,
            path: None,
            commit: None,
        };
        self.write_confirmation(p, link, &c)?;
        self.try_commit();
        Ok(())
    }

    fn write_reject(&mut self, p: PartyIndex, link: Option<Hash>) -> Result<Hash, SimError> {
        let r = Reject {
            v: PROTOCOL_VERSION,
            chain: self.chain_id().clone(),
            seq: self.open.seq,
            round: self.open.round,
            link: link.map(|h| h.to_base64url()).unwrap_or_default(),
            kid: self.parties[p].kid(),
            ts: self.now_ms,
        };
        let bytes =
            sign(&self.parties[p].client, typ::REJECT, &r).map_err(|e| SimError(e.to_string()))?;
        let hash = Hash::of(&bytes);
        let file = format!(
            "{}rejects/{}-r{}.jws",
            self.chain_folder(),
            seq8(r.seq),
            r.round
        );
        self.write_party(p, &file, bytes);
        self.open.votes().cast(p, Vote::Nothing);
        Ok(hash)
    }

    /// `p` refuses a proposal of the current round.
    pub fn reject(&mut self, p: PartyIndex, link: Hash) -> Result<(), SimError> {
        self.guard_vote(p)?;
        match self.open.proposals.get(&link) {
            Some(prop) if prop.link.round == self.open.round => {}
            _ => return err("no such proposal in this round"),
        }
        self.write_reject(p, Some(link))?;
        Ok(())
    }

    /// The designated proposer passes (§6.3).
    pub fn pass(&mut self, p: PartyIndex) -> Result<(), SimError> {
        self.guard_vote(p)?;
        if self.designated() != Some(p) {
            return err("only the designated proposer passes");
        }
        self.write_reject(p, None)?;
        Ok(())
    }

    /// `p` skips a silent designated proposer (§6.3): round ≥ 1, once per seq.
    pub fn skip(&mut self, p: PartyIndex) -> Result<(), SimError> {
        self.guard_vote(p)?;
        if self.open.round == 0 || self.designated() == Some(p) {
            return err("a skip is by a non-designated party in round >= 1");
        }
        if !self.open.skipped.insert(p) {
            return err("one skip per party per seq");
        }
        self.write_reject(p, None)?;
        Ok(())
    }

    /// Enter the next round: honest only once the current one is dead (§6.4).
    pub fn advance_round(&mut self) -> Result<u32, SimError> {
        if !self.round_is_dead() {
            return err(format!(
                "round {} of seq {} is not dead",
                self.open.round, self.open.seq
            ));
        }
        self.open.round += 1;
        Ok(self.open.round)
    }

    /// Commit the current round's QC, if one has formed; mirror it to every party.
    pub fn try_commit(&mut self) -> Option<Hash> {
        let n = self.n();
        let round = self.open.round;
        let votes = self.open.rounds.get(&round)?.clone();
        let winner = self
            .open
            .proposals
            .iter()
            .filter(|(_, p)| p.link.round == round)
            .find(|(h, p)| votes.votes_for(h) >= effective_quorum(p.link.kind(), n, self.q))
            .map(|(h, _)| *h)?;
        let prop = self.open.proposals.remove(&winner).expect("winner");
        let mut qc: Vec<(String, Vec<u8>, Hash)> = self
            .open
            .confirmations
            .iter()
            .filter(|((h, _), _)| *h == winner)
            .map(|(_, v)| v.clone())
            .collect();
        qc.sort_by(|a, b| a.0.cmp(&b.0));

        // Genesis commits: init the rules.
        let state = if self.open.seq == 0 {
            let g = self.genesis.as_ref().expect("genesis");
            let confirmations: Vec<Confirmation> = qc
                .iter()
                .filter_map(|(_, b, _)| {
                    Signed::<Confirmation>::decode(b.clone(), typ::CONFIRM).ok()
                })
                .map(|s| s.payload)
                .collect();
            self.rules.init(g, &confirmations, &[]).ok()
        } else {
            prop.next_state.clone()
        };
        if let Some((secret, grant)) = &prop.rekey {
            let party = &mut self.parties[prop.author];
            party.client = Keypair::from_secret(secret);
            party.grant = grant.clone();
        }

        // Mirror (§7): every party holds the link and its QC.
        let seq = self.open.seq;
        let chain_folder = self.chain_folder();
        for p in 0..n {
            let file = format!("{chain_folder}links/{}-{}.jws", seq8(seq), winner.h16());
            self.write_party(p, &file, prop.bytes.clone());
            for (kid, bytes, _) in &qc {
                let file = format!(
                    "{chain_folder}confirms/{}-{}-{kid}.jws",
                    seq8(seq),
                    winner.h16()
                );
                self.write_party(p, &file, bytes.clone());
            }
        }
        self.committed.push(Head {
            hash: winner,
            bytes: prop.bytes,
            link: prop.link,
            qc,
            state,
        });
        self.open = Open::new(seq + 1);
        Some(winner)
    }

    // ── Witnesses ───────────────────────────────────────────────────────────────────────────

    /// Every witness polls every party folder and receipts what it has not yet seen, per its
    /// behaviour.
    pub fn witnesses_observe(&mut self) {
        let Some(chain) = self.chain.clone() else {
            return;
        };
        let prefix = format!("chains/{chain}/");
        let mut records: Vec<(String, Vec<u8>)> = Vec::new();
        for p in &self.parties {
            let folder = p.folder();
            if let Some(files) = self.storage.folders.get(&folder) {
                for (name, bytes) in files {
                    if name.starts_with(&prefix) {
                        records.push((p.pubky(), bytes.clone()));
                    }
                }
            }
        }
        for w in 0..self.witnesses.len() {
            if self.witnesses[w].engagement.is_none() {
                continue;
            }
            let behaviour = self.witnesses[w].behaviour;
            if behaviour == WitnessBehaviour::Dark {
                continue;
            }
            for (owner, bytes) in &records {
                let hash = Hash::of(bytes);
                if self.receipted[w].contains_key(&hash) {
                    continue;
                }
                let Some((typ, seq, round, by)) = describe(bytes) else {
                    continue;
                };
                if let WitnessBehaviour::Selective(skip) = behaviour {
                    if self
                        .parties
                        .get(skip)
                        .map(|p| p.kid() == by)
                        .unwrap_or(false)
                    {
                        continue;
                    }
                }
                let observed_at = match behaviour {
                    WitnessBehaviour::Skewed(off) => (self.now_ms as i64 + off).max(0) as u64,
                    _ => self.now_ms,
                };
                self.cursor += 1;
                let r = Receipt {
                    v: PROTOCOL_VERSION,
                    kind: "observed".into(),
                    chain: chain.clone(),
                    record: hash.to_base64url(),
                    typ: typ.to_string(),
                    seq: Some(seq),
                    round,
                    by,
                    kid: self.witnesses[w].kid(),
                    observed_at,
                    source: Source {
                        pubky: owner.clone(),
                        cursor: self.cursor,
                    },
                    consistent: true,
                };
                let Ok(rb) = sign(&self.witnesses[w].client, typ::WITNESS, &r) else {
                    continue;
                };
                let rh = Hash::of(&rb);
                if behaviour != WitnessBehaviour::DeletesReceipts {
                    let folder = self.witnesses[w].folder();
                    let file = format!("witness/{chain}/{}-{}.jws", seq8(seq), rh.h16());
                    self.storage.put(&folder, &file, rb.clone());
                }
                self.receipted[w].insert(hash, (observed_at, rb));
            }
        }
    }

    // ── Byzantine helpers ───────────────────────────────────────────────────────────────────

    /// Write a confirmation by `p` for `link` at `(seq, round)` regardless of discipline; the
    /// model is not updated. Returns the record's hash.
    pub fn forge_confirmation(
        &mut self,
        p: PartyIndex,
        seq: u64,
        round: Option<u32>,
        link: Hash,
        state: &str,
    ) -> Hash {
        let c = Confirmation {
            v: PROTOCOL_VERSION,
            chain: self.chain_id().clone(),
            seq,
            round,
            link: link.to_base64url(),
            kid: self.parties[p].kid(),
            ts: self.now_ms,
            state: state.to_string(),
            grant: None,
            path: None,
            commit: None,
        };
        let bytes = sign(&self.parties[p].client, typ::CONFIRM, &c).expect("typ");
        let hash = Hash::of(&bytes);
        let file = format!(
            "{}confirms/{}-{}-forged.jws",
            self.chain_folder(),
            seq8(seq),
            link.h16()
        );
        self.write_party(p, &file, bytes);
        hash
    }

    /// Delete every file `p` wrote whose name matches `predicate`.
    pub fn delete_where(&mut self, p: PartyIndex, predicate: impl Fn(&str) -> bool) {
        let folder = self.parties[p].folder();
        if let Some(files) = self.storage.folders.get_mut(&folder) {
            files.retain(|name, _| !predicate(name));
        }
    }

    // ── Inputs for the fold ─────────────────────────────────────────────────────────────────

    /// Everything currently in the given folders, as the fold wants it.
    pub fn inputs_from(&self, folders: &[&str]) -> Inputs {
        let mut inputs = Inputs {
            chain: self.chain.clone(),
            ..Inputs::default()
        };
        for (folder, name, bytes) in self.storage.iter() {
            if !folders.contains(&folder) {
                continue;
            }
            let segments: Vec<&str> = name.split('/').collect();
            let bucket = match segments.as_slice() {
                ["chains", _, "links", _] => &mut inputs.links,
                ["chains", _, "confirms", _] => &mut inputs.confirms,
                ["chains", _, "rejects", _] => &mut inputs.rejects,
                ["chains", _, "receipts", _, "engage.jws"] => &mut inputs.engagements,
                ["chains", _, "receipts", _, _] => &mut inputs.receipts,
                ["witness", _, "engage.jws"] => &mut inputs.engagements,
                ["witness", _, "engage", _] => &mut inputs.engagements,
                ["witness", _, _] => &mut inputs.receipts,
                ["keys", file] if file.ends_with(".revoked.jws") => &mut inputs.revocations,
                _ => continue,
            };
            bucket.push(bytes.to_vec());
            inputs
                .sources
                .entry(Hash::of(bytes))
                .or_default()
                .push(folder.to_string());
        }
        inputs
    }

    /// Everything in every folder.
    pub fn inputs_all(&self) -> Inputs {
        let folders: Vec<&str> = self.storage.folders.keys().map(String::as_str).collect();
        self.inputs_from(&folders)
    }

    /// Every party folder.
    pub fn party_folders(&self) -> Vec<String> {
        self.parties.iter().map(Party::folder).collect()
    }

    /// Every witness folder.
    pub fn witness_folders(&self) -> Vec<String> {
        self.witnesses.iter().map(Witness::folder).collect()
    }
}

/// What a record is, for a receipt: `(typ, seq, round, signer kid)`.
fn describe(bytes: &[u8]) -> Option<(&'static str, u64, Option<u32>, String)> {
    if let Ok(l) = Signed::<Link>::decode(bytes.to_vec(), typ::LINK) {
        return Some((
            typ::LINK,
            l.payload.seq,
            Some(l.payload.round),
            l.payload.kid,
        ));
    }
    if let Ok(c) = Signed::<Confirmation>::decode(bytes.to_vec(), typ::CONFIRM) {
        return Some((typ::CONFIRM, c.payload.seq, c.payload.round, c.payload.kid));
    }
    if let Ok(r) = Signed::<Reject>::decode(bytes.to_vec(), typ::REJECT) {
        return Some((
            typ::REJECT,
            r.payload.seq,
            Some(r.payload.round),
            r.payload.kid,
        ));
    }
    None
}

// ─── Honest schedules ─────────────────────────────────────────────────────────────────────────

/// What a non-proposer does in round 0 of a planned seq.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Confirm the `i`-th proposer's link (modulo the number of proposers).
    Confirm(usize),
    /// Refuse the `i`-th proposer's link — honest only when there are competing proposals,
    /// so with a single proposal this is read as `Confirm(0)`.
    Reject(usize),
}

/// One seq of honest activity: some parties propose at once in round 0, the rest vote, and if
/// the round dies the designated proposer of each following round passes (`passes` times) or
/// re-proposes the lowest-voted link, which everyone then confirms.
#[derive(Debug, Clone)]
pub struct SeqPlan {
    /// Distinct parties proposing in round 0 (at least one).
    pub proposers: Vec<PartyIndex>,
    /// One `add {n}` value per proposer.
    pub values: Vec<u64>,
    /// One choice per party; ignored for proposers.
    pub choices: Vec<Choice>,
    /// Rounds `1..=passes` end with the designated proposer passing.
    pub passes: u32,
}

impl<R: Rules> Sim<R> {
    /// Run genesis to commitment with every party confirming, then engage the witnesses.
    pub fn bootstrap(&mut self) -> Result<(), SimError> {
        self.genesis()?;
        for p in 1..self.n() {
            self.confirm_genesis(p)?;
            self.tick(1_000);
        }
        if self.committed.is_empty() {
            return err("genesis did not commit");
        }
        self.engage_witnesses()?;
        self.witnesses_observe();
        Ok(())
    }

    /// Play one planned seq honestly until it commits. Returns the number of rounds used.
    pub fn play(&mut self, plan: &SeqPlan) -> Result<u32, SimError> {
        let n = self.n();
        let mut proposers: Vec<PartyIndex> = plan.proposers.iter().map(|p| p % n).collect();
        proposers.dedup();
        let mut seen = BTreeSet::new();
        proposers.retain(|p| seen.insert(*p));
        if proposers.is_empty() {
            return err("a plan needs a proposer");
        }
        let seq = self.open.seq;
        let mut hashes = Vec::new();
        for (i, &p) in proposers.iter().enumerate() {
            let n_val = plan.values.get(i).copied().unwrap_or(1);
            hashes.push(self.propose(p, "add", json!({ "n": n_val }))?);
            self.tick(500);
        }
        self.witnesses_observe();
        for p in 0..n {
            if proposers.contains(&p) {
                continue;
            }
            let choice = plan.choices.get(p).copied().unwrap_or(Choice::Confirm(0));
            match choice {
                Choice::Reject(i) if hashes.len() > 1 => {
                    self.reject(p, hashes[i % hashes.len()])?
                }
                Choice::Reject(_) => self.confirm(p, hashes[0])?,
                Choice::Confirm(i) => self.confirm(p, hashes[i % hashes.len()])?,
            }
            self.tick(500);
            self.witnesses_observe();
            if self.open.seq > seq {
                return Ok(1);
            }
        }
        // Everyone voted and nothing committed: the round is dead by construction.
        let mut rounds = 1;
        loop {
            self.advance_round()?;
            rounds += 1;
            let d = self.designated().expect("round >= 1");
            if self.open.round <= plan.passes {
                self.pass(d)?;
                self.tick(500);
                // Under q < N one pass does not kill an empty round (§6.4: N − q + 1
                // rejects); the others skip, once each per seq, until it is dead.
                for p in 0..n {
                    if self.round_is_dead() {
                        break;
                    }
                    if p != d && !self.open.skipped.contains(&p) {
                        self.skip(p)?;
                        self.tick(500);
                    }
                }
                if !self.round_is_dead() {
                    return err("no skips left to kill an empty round");
                }
                self.witnesses_observe();
                continue;
            }
            let link = match self.lowest_voted_in_previous_round() {
                Some(h) => self.repropose(d, h)?,
                None => self.propose(
                    d,
                    "add",
                    json!({ "n": plan.values.first().copied().unwrap_or(1) }),
                )?,
            };
            self.tick(500);
            self.witnesses_observe();
            for p in 0..n {
                if p == d {
                    continue;
                }
                self.confirm(p, link)?;
                self.tick(500);
                self.witnesses_observe();
                if self.open.seq > seq {
                    return Ok(rounds);
                }
            }
            return err("everyone confirmed and nothing committed");
        }
    }
}
