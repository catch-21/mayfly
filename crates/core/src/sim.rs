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
    sign, CloseBody, CloseReason, Confirmation, Engagement, Link, Policy, Receipt, Reject,
    Revocation, Service, Signed, Source,
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
    /// `RandomTally` only: `BLAKE3(nonces)` drawn from the reveals (§6.6); empty for `Tally`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub seed: String,
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

/// `tally/1` with commit–reveal seat randomisation (§6.6): `init` waits for every party's
/// nonce and folds them into `seed`, so the state hash depends on the reveals.
#[derive(Debug, Default, Clone, Copy)]
pub struct RandomTally;

impl Rules for RandomTally {
    type State = TallyState;
    type Body = TallyBody;

    fn id(&self) -> &'static str {
        "random-tally/1"
    }

    fn reference_hash(&self) -> &'static str {
        "random-tally/1-reference-hash"
    }

    fn init(
        &self,
        genesis: &Genesis,
        _confirmations: &[Confirmation],
        nonces: &[Nonce],
    ) -> Result<TallyState, RulesError> {
        if nonces.len() != genesis.parties.len() {
            return Err(RulesError(format!(
                "{} nonces for {} parties",
                nonces.len(),
                genesis.parties.len()
            )));
        }
        let mut all = Vec::new();
        for n in nonces {
            all.extend_from_slice(n);
        }
        Ok(TallyState {
            parties: genesis.parties.len(),
            seed: Hash::of(&all).to_base64url(),
            ..TallyState::default()
        })
    }

    fn wants_reveals(&self, _genesis: &Genesis) -> bool {
        true
    }

    fn obliged(&self, state: &TallyState) -> Vec<PartyIndex> {
        Tally.obliged(state)
    }

    fn may_append(&self, state: &TallyState, party: PartyIndex, kind: &str) -> bool {
        Tally.may_append(state, party, kind)
    }

    fn apply(&self, state: &TallyState, link: &Link) -> Result<TallyState, RulesError> {
        Tally.apply(state, link)
    }

    fn status(&self, state: &TallyState) -> Status {
        Tally.status(state)
    }

    fn close(&self, state: &TallyState, close: &CloseBody) -> Result<Outcome, RulesError> {
        Tally.close(state, close)
    }

    fn canonical_state(&self, state: &TallyState) -> Vec<u8> {
        Tally.canonical_state(state)
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
    /// The nonce this party commits to at genesis and reveals later (§6.6).
    pub nonce: [u8; 16],
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

/// A `rekey` or `recover` pending on a proposal: applied to the party on commit.
#[derive(Clone)]
struct KeyChange {
    secret: [u8; 32],
    grant: String,
    /// `recover` only: the new app and folder.
    client_id: Option<String>,
    path: Option<String>,
}

/// A proposal in the open seq.
#[derive(Clone)]
struct Proposal<S> {
    bytes: Vec<u8>,
    link: Link,
    author: PartyIndex,
    next_state: Option<S>,
    key_change: Option<KeyChange>,
    /// `reveal`: the nonce.
    reveal: Option<Vec<u8>>,
    /// `witnesses`: witness indices to seat and unseat on commit.
    witness_change: Option<(Vec<usize>, Vec<usize>)>,
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
    /// Every skip made: `(seq, round, skipper)`.
    pub skip_log: Vec<(u64, u32, PartyIndex)>,
    /// Witness indices currently engaged: genesis witnesses, then as `witnesses` links change
    /// it.
    pub engaged: BTreeSet<usize>,
    /// Nonces revealed so far, by party.
    pub revealed: BTreeMap<PartyIndex, Vec<u8>>,
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
                    nonce: pubky_common::crypto::random_bytes::<16>(),
                }
            })
            .collect();
        let witnesses = (0..k).map(|_| Self::new_witness(now_s)).collect();
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
            skip_log: Vec::new(),
            engaged: (0..k).collect(),
            revealed: BTreeMap::new(),
            genesis: None,
            genesis_bytes: None,
            receipted: vec![BTreeMap::new(); k],
            cursor: 0,
        }
    }

    fn new_witness(now_s: u64) -> Witness {
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
    }

    /// Before genesis: re-mint party `p`'s Grant with a lifetime of `lifetime_secs`, so a test
    /// can walk past its `exp` (§9.2 step 4).
    pub fn reissue_grant(&mut self, p: PartyIndex, lifetime_secs: u64) {
        let party = &self.parties[p];
        let now_s = self.now_ms / 1000;
        let claims = GrantClaims {
            iss: party.identity.public_key(),
            client_id: ClientId::new(&party.client_id).expect("client id"),
            caps: vec![Capability::read_write(format!("/pub/{}/", party.client_id)).expect("cap")],
            cnf: party.client.public_key(),
            jti: GrantId::generate(),
            iat: now_s,
            exp: now_s + lifetime_secs,
        };
        let signed = claims.sign(&self.parties[p].identity, GRANT_JWS_TYP);
        self.parties[p].grant = signed;
    }

    /// A new witness, not named at genesis, with keys and — once the chain exists — an
    /// `engage.jws`. Returns its index; seat it with [`Sim::propose_witnesses`].
    pub fn add_witness(&mut self, behaviour: WitnessBehaviour) -> Result<usize, SimError> {
        let mut w = Self::new_witness(self.now_ms / 1000);
        w.behaviour = behaviour;
        self.witnesses.push(w);
        self.receipted.push(BTreeMap::new());
        let i = self.witnesses.len() - 1;
        if self.chain.is_some() {
            self.engage_witness(i)?;
        }
        Ok(i)
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
                    path: Some(p.path.clone()),
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
                key_change: None,
                reveal: None,
                witness_change: None,
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
            commit: self
                .genesis
                .as_ref()
                .filter(|g| self.rules.wants_reveals(g))
                .map(|_| Hash::of(&party.nonce).to_base64url()),
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
        for i in 0..self.witnesses.len() {
            if self.witnesses[i].engagement.is_none() {
                self.engage_witness(i)?;
            }
        }
        Ok(())
    }

    fn engage_witness(&mut self, i: usize) -> Result<(), SimError> {
        let chain = self.chain_id().clone();
        let parties: Vec<String> = self.parties.iter().map(Party::pubky).collect();
        let w = &self.witnesses[i];
        let e = Engagement {
            v: PROTOCOL_VERSION,
            kind: "engage".into(),
            chain: chain.clone(),
            kid: w.kid(),
            grant: w.grant.clone(),
            path: w.path.clone(),
            parties,
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
            if !self.engaged.contains(&i) {
                continue; // an honest client embeds only engaged witnesses' receipts
            }
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
        key_change: Option<KeyChange>,
    ) -> Result<Hash, SimError> {
        self.write_proposal_full(p, link, next_state, key_change, None, None)
    }

    fn write_proposal_full(
        &mut self,
        p: PartyIndex,
        link: Link,
        next_state: Option<R::State>,
        key_change: Option<KeyChange>,
        reveal: Option<Vec<u8>>,
        witness_change: Option<(Vec<usize>, Vec<usize>)>,
    ) -> Result<Hash, SimError> {
        // A `recover` is signed by the new key it introduces (§6.7); everything else by the
        // party's established key.
        let signer = match (&key_change, link.kind()) {
            (Some(kc), crate::record::Kind::Recover) => Keypair::from_secret(&kc.secret),
            _ => self.parties[p].client.clone(),
        };
        let bytes = sign(&signer, typ::LINK, &link).map_err(|e| SimError(e.to_string()))?;
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
                key_change,
                reveal,
                witness_change,
            },
        );
        Ok(hash)
    }

    /// `p` reveals the nonce it committed to at genesis (§6.6). Honest only for the next
    /// unrevealed party in genesis order.
    pub fn propose_reveal(&mut self, p: PartyIndex) -> Result<Hash, SimError> {
        self.guard_proposer(p)?;
        let next = (1..self.n()).find(|q| !self.revealed.contains_key(q));
        if next != Some(p) {
            return err(format!("the next reveal is {next:?}'s, not {p}'s"));
        }
        let nonce = self.parties[p].nonce.to_vec();
        let body = json!({ "nonce": b64url(&nonce) });
        let link = self.build_link(p, self.open.round, "reveal", body, None);
        self.write_proposal_full(p, link, None, None, Some(nonce), None)
    }

    /// `p` proposes a `witnesses` change (§11.2): seat the witnesses at `add` (which must have
    /// engaged) and unseat those at `remove`.
    pub fn propose_witnesses(
        &mut self,
        p: PartyIndex,
        add: &[usize],
        remove: &[usize],
    ) -> Result<Hash, SimError> {
        self.guard_proposer(p)?;
        if add.is_empty() && remove.is_empty() {
            return err("a witnesses link changes something");
        }
        let mut adds = Vec::new();
        for &w in add {
            let witness = self
                .witnesses
                .get(w)
                .ok_or_else(|| SimError("no such witness".into()))?;
            let engage = witness
                .engagement
                .clone()
                .ok_or_else(|| SimError("that witness has not engaged".into()))?;
            adds.push(json!({ "pubky": witness.pubky(), "kid": witness.kid(), "engage": engage }));
        }
        let removes: Vec<String> = remove.iter().map(|w| self.witnesses[*w].kid()).collect();
        let body = json!({ "add": adds, "remove": removes });
        let link = self.build_link(p, self.open.round, "witnesses", body, None);
        let state = self.head().and_then(|h| h.state.clone());
        self.write_proposal_full(
            p,
            link,
            state,
            None,
            None,
            Some((add.to_vec(), remove.to_vec())),
        )
    }

    /// `p` disowns its current chain key (§5.3): a fresh key and Grant under the same identity
    /// sign `keys/<kid>.revoked.jws`. The party keeps using the revoked key — that is what the
    /// test wants to see caught. Returns the revocation's hash.
    pub fn revoke_key(&mut self, p: PartyIndex) -> Hash {
        let party = &self.parties[p];
        let other = Keypair::random();
        let grant = mint_grant(
            &party.identity,
            &other,
            &party.client_id,
            self.now_ms / 1000,
        );
        let r = Revocation {
            v: PROTOCOL_VERSION,
            kind: "revoked".into(),
            revoked: party.kid(),
            by: other.public_key().z32(),
            grant,
            ts: self.now_ms,
        };
        let bytes = sign(&other, typ::REVOKE, &r).expect("typ");
        let hash = Hash::of(&bytes);
        let file = format!("keys/{}.revoked.jws", r.revoked);
        self.write_party(p, &file, bytes);
        hash
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
        self.write_proposal(
            p,
            link,
            state,
            Some(KeyChange {
                secret: new_client.secret(),
                grant,
                client_id: None,
                path: None,
            }),
        )
    }

    /// `p` proposes `recover` to a fresh key under `new_client_id` (§6.7): signed by the new
    /// key, moving the party's folder to `/pub/<new_client_id>/mayfly/` from the next seq.
    /// Returns the link hash; the old key stays in `parties[p]` until it commits.
    pub fn propose_recover(
        &mut self,
        p: PartyIndex,
        new_client_id: &str,
    ) -> Result<Hash, SimError> {
        self.guard_proposer(p)?;
        let new_client = Keypair::random();
        let party = &self.parties[p];
        let grant = mint_grant(
            &party.identity,
            &new_client,
            new_client_id,
            self.now_ms / 1000,
        );
        let path = format!("/pub/{new_client_id}/{PROTOCOL_FOLDER}/");
        let body = json!({ "new_kid": new_client.public_key().z32(), "path": path });
        let mut link = self.build_link(p, self.open.round, "recover", body, Some(grant.clone()));
        link.kid = new_client.public_key().z32();
        let state = self.head().and_then(|h| h.state.clone());
        self.write_proposal(
            p,
            link,
            state,
            Some(KeyChange {
                secret: new_client.secret(),
                grant,
                client_id: Some(new_client_id.to_string()),
                path: Some(path),
            }),
        )
    }

    /// `p` proposes `close {abandoned}` naming `subjects` (§6.8). Judged outside rounds: not a
    /// vote, so the model's open round is untouched. Returns the close's hash.
    pub fn propose_abandoned(
        &mut self,
        p: PartyIndex,
        subjects: &[PartyIndex],
    ) -> Result<Hash, SimError> {
        if self.committed.is_empty() {
            return err("genesis has not committed");
        }
        if subjects.contains(&p) || subjects.is_empty() {
            return err("a close names others, and at least one");
        }
        let body = CloseBody {
            reason: CloseReason::Abandoned,
            subject: subjects.iter().map(|s| self.parties[*s].pubky()).collect(),
            pending: self
                .open
                .current_proposals()
                .iter()
                .map(Hash::to_base64url)
                .collect(),
        };
        let link = self.build_link(
            p,
            0,
            "close",
            serde_json::to_value(&body).expect("close body"),
            None,
        );
        let bytes =
            sign(&self.parties[p].client, typ::LINK, &link).map_err(|e| SimError(e.to_string()))?;
        let hash = Hash::of(&bytes);
        let file = format!(
            "{}links/{}-{}.jws",
            self.chain_folder(),
            seq8(link.seq),
            hash.h16()
        );
        self.write_party(p, &file, bytes);
        Ok(hash)
    }

    /// `p` confirms an abandoned close: a confirmation with no `round` (§6.8).
    pub fn confirm_abandoned(&mut self, p: PartyIndex, close: Hash) -> Result<Hash, SimError> {
        let head = self
            .head()
            .ok_or_else(|| SimError("genesis has not committed".into()))?;
        let c = Confirmation {
            v: PROTOCOL_VERSION,
            chain: self.chain_id().clone(),
            seq: self.open.seq,
            round: None,
            link: close.to_base64url(),
            kid: self.parties[p].kid(),
            ts: self.now_ms,
            state: head.link.state.clone(),
            grant: None,
            path: None,
            commit: None,
        };
        let bytes =
            sign(&self.parties[p].client, typ::CONFIRM, &c).map_err(|e| SimError(e.to_string()))?;
        let hash = Hash::of(&bytes);
        let file = format!(
            "{}confirms/{}-{}.jws",
            self.chain_folder(),
            seq8(c.seq),
            close.h16()
        );
        self.write_party(p, &file, bytes);
        Ok(hash)
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
        self.skip_log.push((self.open.seq, self.open.round, p));
        Ok(())
    }

    /// `p` votes for nothing in a round where the designated proposer has already passed
    /// (§6.3): an ordinary empty reject, not a skip, so it spends no skip budget. Under
    /// `q < N` this is how the remaining `N − q` rejects that kill an empty round arrive.
    pub fn empty_reject(&mut self, p: PartyIndex) -> Result<(), SimError> {
        self.guard_vote(p)?;
        let Some(d) = self.designated() else {
            return err("round 0 has no designated proposer to have passed");
        };
        let designated_passed = self
            .open
            .rounds
            .get(&self.open.round)
            .map(|r| {
                r.by_party
                    .get(&d)
                    .is_some_and(|v| v.contains(&Vote::Nothing))
            })
            .unwrap_or(false);
        if !designated_passed {
            return err("the designated proposer has not passed; that would be a skip");
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
            if self.rules.wants_reveals(g) {
                None
            } else {
                self.rules.init(g, &confirmations, &[]).ok()
            }
        } else if let Some(nonce) = &prop.reveal {
            // A reveal: when the last is in, the rules initialise (§6.6).
            self.revealed.insert(prop.author, nonce.clone());
            if (1..n).all(|p| self.revealed.contains_key(&p)) {
                let g = self.genesis.as_ref().expect("genesis");
                let genesis_nonce = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(&g.nonce)
                    .expect("genesis nonce");
                let mut nonces = vec![genesis_nonce];
                for p in 1..n {
                    nonces.push(self.revealed[&p].clone());
                }
                let confirmations: Vec<Confirmation> = self.committed[0]
                    .qc
                    .iter()
                    .filter_map(|(_, b, _)| {
                        Signed::<Confirmation>::decode(b.clone(), typ::CONFIRM).ok()
                    })
                    .map(|s| s.payload)
                    .collect();
                self.rules.init(g, &confirmations, &nonces).ok()
            } else {
                None
            }
        } else {
            prop.next_state.clone()
        };
        if let Some((add, remove)) = &prop.witness_change {
            for w in add {
                self.engaged.insert(*w);
            }
            for w in remove {
                self.engaged.remove(w);
            }
        }
        if let Some(kc) = &prop.key_change {
            let party = &mut self.parties[prop.author];
            party.client = Keypair::from_secret(&kc.secret);
            party.grant = kc.grant.clone();
            if let Some(client_id) = &kc.client_id {
                party.client_id = client_id.clone();
            }
            if let Some(path) = &kc.path {
                party.path = path.clone();
            }
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
                    // Chain records, and `keys/<kid>.revoked.jws` (§11.3).
                    if name.starts_with(&prefix)
                        || (name.starts_with("keys/") && name.ends_with(".revoked.jws"))
                    {
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
                    seq,
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
                    let file = match seq {
                        Some(seq) => format!("witness/{chain}/{}-{}.jws", seq8(seq), rh.h16()),
                        None => format!("witness/{chain}/revoked-{}-{}.jws", r.by, rh.h16()),
                    };
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

    /// Write a reject into `p`'s folder signed by `signer` — any key, so a test can veto with
    /// an old key after a recover, or skip a second time. The model is not updated.
    pub fn forge_reject(
        &mut self,
        p: PartyIndex,
        signer: &Keypair,
        seq: u64,
        round: u32,
        link: Option<Hash>,
    ) -> Hash {
        let r = Reject {
            v: PROTOCOL_VERSION,
            chain: self.chain_id().clone(),
            seq,
            round,
            link: link.map(|h| h.to_base64url()).unwrap_or_default(),
            kid: signer.public_key().z32(),
            ts: self.now_ms,
        };
        let bytes = sign(signer, typ::REJECT, &r).expect("typ");
        let hash = Hash::of(&bytes);
        let file = format!(
            "{}rejects/{}-r{round}-{}.jws",
            self.chain_folder(),
            seq8(seq),
            hash.h16()
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

/// What a record is, for a receipt: `(typ, seq, round, signer kid)`. A revocation has no seq
/// and its `by` is the revoked kid (§11.3).
fn describe(bytes: &[u8]) -> Option<(&'static str, Option<u64>, Option<u32>, String)> {
    if let Ok(l) = Signed::<Link>::decode(bytes.to_vec(), typ::LINK) {
        return Some((
            typ::LINK,
            Some(l.payload.seq),
            Some(l.payload.round),
            l.payload.kid,
        ));
    }
    if let Ok(c) = Signed::<Confirmation>::decode(bytes.to_vec(), typ::CONFIRM) {
        return Some((
            typ::CONFIRM,
            Some(c.payload.seq),
            c.payload.round,
            c.payload.kid,
        ));
    }
    if let Ok(r) = Signed::<Reject>::decode(bytes.to_vec(), typ::REJECT) {
        return Some((
            typ::REJECT,
            Some(r.payload.seq),
            Some(r.payload.round),
            r.payload.kid,
        ));
    }
    if let Ok(r) = Signed::<Revocation>::decode(bytes.to_vec(), typ::REVOKE) {
        return Some((typ::REVOKE, None, None, r.payload.revoked));
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
///
/// A party whose behaviour is `WalksAwayAt(s)` with `s <= seq` does nothing at all; when it is
/// designated, `SkipsEarly` parties skip it (after `silent_wait_ms`), once each per seq.
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
    /// How long skippers wait before skipping a silent designated proposer.
    pub silent_wait_ms: u64,
}

impl SeqPlan {
    /// One proposer, everyone confirms.
    pub fn single(proposer: PartyIndex, n: usize, value: u64) -> Self {
        Self {
            proposers: vec![proposer],
            values: vec![value],
            choices: vec![Choice::Confirm(0); n],
            passes: 0,
            silent_wait_ms: 0,
        }
    }
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
        let silent = |sim: &Self, p: PartyIndex| matches!(sim.parties[p].behaviour, Behaviour::WalksAwayAt(s) if s <= seq);
        proposers.retain(|p| !silent(self, *p));
        if proposers.is_empty() {
            return err("every proposer in the plan has walked away");
        }
        let mut hashes = Vec::new();
        for (i, &p) in proposers.iter().enumerate() {
            let n_val = plan.values.get(i).copied().unwrap_or(1);
            hashes.push(self.propose(p, "add", json!({ "n": n_val }))?);
            self.tick(500);
        }
        self.witnesses_observe();
        for p in 0..n {
            if proposers.contains(&p) || silent(self, p) {
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
        // Everyone who is present voted and nothing committed: the round is dead unless a
        // silent party's missing vote is what keeps it alive, which no honest party can fix
        // in round 0 (§6.3: no skips there).
        if !self.round_is_dead() {
            return err("round 0 is alive only for a silent party's vote: the seq is stuck");
        }
        let mut rounds = 1;
        loop {
            self.advance_round()?;
            rounds += 1;
            let d = self.designated().expect("round >= 1");
            if silent(self, d) {
                // A silent designated proposer: `SkipsEarly` parties skip it (§6.3), after
                // waiting `silent_wait_ms`, until the round is dead or they run out.
                self.tick(plan.silent_wait_ms);
                for p in 0..n {
                    if self.round_is_dead() {
                        break;
                    }
                    if p != d
                        && !silent(self, p)
                        && self.parties[p].behaviour == Behaviour::SkipsEarly
                        && !self.open.skipped.contains(&p)
                    {
                        self.skip(p)?;
                        self.tick(500);
                        self.witnesses_observe();
                    }
                }
                if !self.round_is_dead() {
                    return err("a silent designated proposer and nobody left to skip them");
                }
                continue;
            }
            if self.open.round <= plan.passes {
                self.pass(d)?;
                self.tick(500);
                // Under q < N one pass does not kill an empty round (§6.4: N − q + 1
                // rejects); the others vote for nothing too until it is dead. The designated
                // party has acted, so these are ordinary empty rejects, not skips (§6.3).
                for p in 0..n {
                    if self.round_is_dead() {
                        break;
                    }
                    if p != d && !silent(self, p) {
                        self.empty_reject(p)?;
                        self.tick(500);
                    }
                }
                if !self.round_is_dead() {
                    return err("everyone voted for nothing and the round is not dead");
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
                if p == d || silent(self, p) {
                    continue;
                }
                self.confirm(p, link)?;
                self.tick(500);
                self.witnesses_observe();
                if self.open.seq > seq {
                    return Ok(rounds);
                }
            }
            if self.round_is_dead() {
                continue;
            }
            return err("everyone present voted and the round is neither committed nor dead");
        }
    }
}
