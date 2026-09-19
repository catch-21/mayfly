//! The verifier (§9.2): anyone, from files alone.
//!
//! Inputs are bytes the caller found — links, confirmations, rejects, engagements, receipts,
//! revocations — from any source and in any order. Output is a [`Verdict`]: the committed chain,
//! its status, and every anomaly attributed to a key. The fold never trusts where a file came
//! from; it trusts signatures, hashes and quorums.
//!
//! The two principles every step obeys (§6.4, §11.2):
//!
//! 1. **Commitment consults votes only.** Step 3d reads proposals, confirmations and rejects.
//!    Death evidence, receipts and skips' fairness are reported, never inputs to which link is
//!    committed.
//! 2. **Nothing waits for a third party.** No receipt is a validity condition. Every time
//!    verdict is provisional until the records it touches are final, and is reached by the
//!    witness quorum, never one receipt.
//!
//! The algorithm is written as the specification's numbered steps so that a reader can hold
//! the two side by side. Step 4 (Grant windows) removes records the witness quorum places
//! outside their key's window and folds again; [`verify`] is that loop.
//!
//! Not modelled: an engagement lapsing at `until` (the fold has no clock of its own to read
//! it against) and the `mirror` service tier's byte-for-byte comparison (`TamperedMirror`).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use base64::Engine;
use pubky_common::crypto::PublicKey;

use crate::close::{max_subjects, CloseState};
use crate::error::Error;
use crate::genesis::Genesis;
use crate::hash::{ChainId, Hash};
use crate::keys::{parse_z32, verify_grant, Seat};
use crate::record::{
    CloseBody, CloseReason, Confirmation, Engagement, KeyChangeBody, Kind, Link, Receipt, Reject,
    RevealBody, Revocation, Signed, WitnessChange,
};
use crate::rules::{state_hash, Outcome, PartyIndex, Rules, Status as RulesStatus};
use crate::typ;
use crate::vote::{designated, effective_quorum, RoundVotes, Vote};
use crate::witness::{adjudicate, Engaged, Receipts, Verdict as TimeVerdict};
use crate::PROTOCOL_FOLDER;

/// Everything the caller could find. Bytes only; the fold decodes and verifies.
#[derive(Debug, Default, Clone)]
pub struct Inputs {
    /// Fold only this chain. Required when the inputs hold more than one valid genesis.
    pub chain: Option<ChainId>,
    /// Contents of every `links/*.jws` from every declared path, plus any watchdog mirror.
    pub links: Vec<Vec<u8>>,
    /// Contents of every `confirms/*.jws`.
    pub confirms: Vec<Vec<u8>>,
    /// Contents of every `rejects/*.jws`.
    pub rejects: Vec<Vec<u8>>,
    /// Every `engage.jws` found: current, historic, and party-mirrored.
    pub engagements: Vec<Vec<u8>>,
    /// Every receipt found: on witnesses, mirrored by parties, embedded in links.
    pub receipts: Vec<Vec<u8>>,
    /// Every `keys/<kid>.revoked.jws` found, capped per established kid (§9.2 step 2).
    pub revocations: Vec<Vec<u8>>,
    /// Which folders each byte-blob was read from, for hostile-source accounting and for the
    /// initiator's path. Optional; a folder is `pubky://<owner>/pub/<client_id>/<folder>/`.
    pub sources: BTreeMap<Hash, Vec<String>>,
}

/// Where the chain stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Head is committed and the next seq is open.
    Ongoing,
    /// No QC at `seq` and no close; reports the current round and who has not voted.
    Stalled {
        /// The open seq.
        seq: u64,
        /// Highest round with any vote.
        round: u32,
        /// Whether that round is dead in the files.
        dead: bool,
        /// Parties who have not voted in it.
        awaiting: Vec<PartyIndex>,
    },
    /// An asserted or contested abandoned close sits at `seq`; provisional.
    Paused {
        /// The seq of the close.
        seq: u64,
        /// Its state.
        close: CloseState,
    },
    /// Ended by `close {finished}` or `close {agreed}`.
    Closed(Outcome),
    /// Ended by an adjudicated `close {abandoned}`.
    Abandoned {
        /// Who was closed out.
        subjects: Vec<PartyIndex>,
        /// The rules' outcome.
        outcome: Outcome,
    },
}

/// A committed link with what the fold knows about it.
#[derive(Debug, Clone)]
pub struct Committed {
    /// The link.
    pub link: Signed<Link>,
    /// Its author.
    pub author: PartyIndex,
    /// Confirmations forming its QC (for an adjudicated abandoned close, the non-subjects').
    pub qc: Vec<Signed<Confirmation>>,
    /// True once a successor has embedded this QC (§6.4).
    pub is_final: bool,
    /// *Witnessed m/k*: how many of the engaged witnesses receipted every confirmation of its
    /// QC (and so its QC-completing one), over how many were engaged.
    pub witnessed: (usize, usize),
}

/// A `recover` the fold accepted, reported as such (§6.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovery {
    /// The seq of the recover link.
    pub seq: u64,
    /// Whose seat changed hands.
    pub party: PartyIndex,
    /// The key it moved to.
    pub new_kid: String,
    /// The recover link.
    pub link: Hash,
    /// The witness quorum's verdict on `recovery_delay_ms` (§9.2 step 3f): `Yes` honoured,
    /// `Asserted` unproven.
    pub delay: TimeVerdict,
}

/// Misbehaviour or inconsistency, attributed. Never stops the fold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anomaly {
    /// Key (party `kid` or witness `kid`) the anomaly is attributed to; `None` if a source.
    pub against: Option<String>,
    /// Seq it concerns, if any.
    pub seq: Option<u64>,
    /// What happened.
    pub kind: AnomalyKind,
    /// Hashes of the records that prove it.
    pub evidence: Vec<Hash>,
}

/// The catalogue of §9.2 step 5 and §11.6.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnomalyKind {
    /// Two votes by one key in one round.
    Equivocation,
    /// Mirrored bytes differ from the original.
    TamperedMirror,
    /// A valid proposal nobody confirmed.
    UnconfirmedProposal,
    /// A round `> 0` whose predecessor is not dead in the files.
    UnjustifiedRound,
    /// A skip before the designated party had been silent for `think_ms` (receipts only).
    PrematureSkip,
    /// An empty reject that is not a vote: a skip in round 0, or a second skip at one seq.
    InvalidSkip,
    /// A vote at a seq that is history, outside the QC its successor embedded (§9.2 step 3d).
    LateVote,
    /// A reject of a link the rules find valid.
    Obstruction,
    /// A confirmation whose `state` differs from the link's.
    RulesDivergence,
    /// An abandoned close that exceeded the subject bound, named its author, or was adjudicated
    /// invalid.
    VoidClose,
    /// A subject record at the seq of an adjudicated-valid close.
    LateSubject,
    /// A recover confirmed inside `recovery_delay_ms` (quorum-adjudicated); against a confirmer.
    PrematureRecover,
    /// A provisional recover voided by its seat's established key (§6.7); against the new key.
    VetoedRecover,
    /// An old-key reject after the recover became history.
    LateVeto,
    /// A record a witness quorum places after its Grant's `exp` or a revocation.
    GrantWindow,
    /// A folder past the per-signer bound; read no further.
    HostileSource,
    /// A receipt embedded in a link that does not verify, or names an engagement not yet
    /// established (§9.2 step 3e).
    BadEmbeddedReceipt,
    /// A witness that never receipted, receipted selectively, deleted a receipt, or contradicts
    /// other witnesses (§11.6 table).
    Witness(String),
    /// Two engagements for one witness pubky with contradictory `until`.
    WitnessEquivocation,
}

/// The fold's output.
#[derive(Debug, Clone)]
pub struct Verdict {
    /// Chain id derived from genesis bytes.
    pub chain: ChainId,
    /// Committed links, seq 0 upward.
    pub committed: Vec<Committed>,
    /// Where the chain stands.
    pub status: Status,
    /// Every seat established at the head, in genesis order (unseated parties omitted).
    pub seats: Vec<Seat>,
    /// Every witness engaged at the head.
    pub engaged: Vec<Engaged>,
    /// Every accepted `recover`.
    pub recoveries: Vec<Recovery>,
    /// Every anomaly, attributed.
    pub anomalies: Vec<Anomaly>,
}

impl Verdict {
    /// The committed links' hashes, seq 0 upward — what two verifiers must agree on.
    pub fn committed_hashes(&self) -> Vec<Hash> {
        self.committed.iter().map(|c| c.link.hash).collect()
    }

    /// The head, if anything is committed.
    pub fn head(&self) -> Option<&Committed> {
        self.committed.last()
    }

    /// Is the status final — a closed or adjudicated-abandoned chain?
    pub fn is_final(&self) -> bool {
        matches!(self.status, Status::Closed(_) | Status::Abandoned { .. })
    }
}

/// Configuration the verifier itself owns (not genesis parameters).
#[derive(Debug, Clone)]
pub struct Config {
    /// Largest record this verifier will read regardless of genesis (§6.6).
    pub max_body_cap: u64,
    /// Per signer, per `(seq, round)`: records beyond this mark the source hostile (§9.2).
    pub records_per_signer_round: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_body_cap: crate::genesis::DEFAULT_MAX_BODY_BYTES,
            records_per_signer_round: 3,
        }
    }
}

/// Verify a chain from files alone.
///
/// `rules` must implement the id genesis pins, or the genesis is invalid (§6.6).
pub fn verify<R: Rules>(rules: &R, inputs: &Inputs, config: &Config) -> Result<Verdict, Error> {
    // Step 4: a record a witness quorum places outside its Grant window, that nothing places
    // inside it and that no committed successor has embedded, is INVALID: remove it and fold
    // again. Each pass removes at least one record, so this terminates.
    let mut excluded: HashSet<Hash> = HashSet::new();
    let mut carried: Vec<Anomaly> = Vec::new();
    loop {
        let mut fold = Fold::new(rules, inputs, config, &excluded)?;
        fold.run()?;
        let invalid = fold.grant_windows();
        if invalid.is_empty() {
            let mut verdict = fold.finish();
            carried.append(&mut verdict.anomalies);
            verdict.anomalies = carried;
            return Ok(verdict);
        }
        carried.extend(
            fold.anomalies
                .iter()
                .filter(|a| a.kind == AnomalyKind::GrantWindow)
                .cloned(),
        );
        excluded.extend(invalid);
    }
}

// ─── Decoding ─────────────────────────────────────────────────────────────────────────────────

/// Decode and index one link's bytes. Exposed so the client can run the single-link check
/// (§9.3) without a full fold.
pub fn decode_link(bytes: Vec<u8>) -> Result<Signed<Link>, Error> {
    Signed::decode(bytes, typ::LINK)
}

/// Decode a confirmation.
pub fn decode_confirmation(bytes: Vec<u8>) -> Result<Signed<Confirmation>, Error> {
    Signed::decode(bytes, typ::CONFIRM)
}

/// Decode a reject.
pub fn decode_reject(bytes: Vec<u8>) -> Result<Signed<Reject>, Error> {
    Signed::decode(bytes, typ::REJECT)
}

/// Decode a receipt.
pub fn decode_receipt(bytes: Vec<u8>) -> Result<Signed<Receipt>, Error> {
    Signed::decode(bytes, typ::WITNESS)
}

/// Decode an engagement.
pub fn decode_engagement(bytes: Vec<u8>) -> Result<Signed<Engagement>, Error> {
    Signed::decode(bytes, typ::WITNESS)
}

/// Decode a pile of bytes into records, dropping duplicates (by hash), oversize files and
/// anything that does not decode. Undecodable bytes are not records; they are not reported.
fn decode_all<T: serde::de::DeserializeOwned>(
    blobs: &[Vec<u8>],
    expected_typ: &str,
    cap: u64,
    seen: &mut HashSet<Hash>,
) -> Vec<Signed<T>> {
    let mut out = Vec::new();
    for b in blobs {
        if b.len() as u64 > cap {
            continue;
        }
        let h = Hash::of(b);
        if !seen.insert(h) {
            continue;
        }
        if let Ok(s) = Signed::<T>::decode(b.clone(), expected_typ) {
            out.push(s);
        }
    }
    out
}

/// The storage path part of a source folder: `pubky://<owner>/pub/x/` → `/pub/x/`.
fn storage_path(source: &str) -> &str {
    match source.strip_prefix("pubky://") {
        Some(rest) => rest.find('/').map(|i| &rest[i..]).unwrap_or("/"),
        None => source,
    }
}

// ─── The fold ─────────────────────────────────────────────────────────────────────────────────

/// A `rekey` or `recover` to apply to its author's seat on commit.
#[derive(Debug, Clone)]
struct KeyChange {
    kid: String,
    exp: u64,
    /// `recover` only: the new app and folder.
    client_id: Option<String>,
    path: Option<String>,
}

/// A `witnesses` link's verified change: engagements to seat and kids to unseat.
type WitnessSet = (Vec<(Engaged, Signed<Engagement>)>, Vec<String>);

/// A link that passed step 3a at its seq.
#[derive(Clone)]
struct Candidate<S> {
    link: Signed<Link>,
    author: PartyIndex,
    kind: Kind,
    /// Rules kinds: the state after `apply`. Protocol kinds and genesis: `None`.
    next_state: Option<S>,
    /// The quorum this kind needs (§9.2 step 3b).
    quorum: usize,
    /// `rekey` / `recover`: the seat change to establish on commit.
    key_change: Option<KeyChange>,
    /// `reveal`: the nonce, already checked against the author's `commit`.
    reveal: Option<Vec<u8>>,
    /// `witnesses`: engagements to seat (verified) and kids to unseat, from the next seq.
    witnesses: Option<WitnessSet>,
    /// The link's embedded QC of `prev`, decoded and verified (empty for genesis).
    embedded_qc: Vec<Signed<Confirmation>>,
}

/// Everything step 3 learned about one seq before deciding it.
struct SeqView<S> {
    candidates: Vec<Candidate<S>>,
    rounds: BTreeMap<u32, RoundVotes>,
    /// Parties with any valid record at this seq, for presence (§6.8).
    present: BTreeSet<PartyIndex>,
    /// Every valid record at this seq by party, for the stopwatch.
    records: BTreeMap<PartyIndex, Vec<Hash>>,
    /// Established-key rejects at a seq holding a `recover` for that seat (§6.3, §6.7).
    vetoes: BTreeMap<PartyIndex, Vec<Hash>>,
    /// Valid skips: `(skipper, round, hash)`.
    skips: Vec<(PartyIndex, u32, Hash)>,
}

/// How step 3d decided a seq.
enum Decision {
    /// A successor with a QC embeds this QC: history.
    History {
        index: usize,
        qc: Vec<Signed<Confirmation>>,
    },
    /// The QC in the highest round: the provisional head.
    Head {
        index: usize,
        qc: Vec<Signed<Confirmation>>,
    },
    /// No QC.
    None,
}

struct Fold<'a, R: Rules> {
    rules: &'a R,
    chain: ChainId,
    genesis: Genesis,
    genesis_link: Signed<Link>,
    n: usize,
    q: usize,
    /// Seats by party index; `None` until a genesis confirmation establishes one.
    seats: Vec<Option<Seat>>,
    /// Party identity keys.
    pubkeys: Vec<PublicKey>,
    /// `kid → party` valid for records at each seq: `keys_at[s]`.
    keys_at: Vec<BTreeMap<String, PartyIndex>>,
    links_by_seq: BTreeMap<u64, Vec<Signed<Link>>>,
    confirms_by_seq: BTreeMap<u64, Vec<Signed<Confirmation>>>,
    rejects_by_seq: BTreeMap<u64, Vec<Signed<Reject>>>,
    engaged: Vec<Engaged>,
    /// Every engagement key ever established, by `kid`.
    engagement_keys: BTreeMap<String, PublicKey>,
    /// Every decoded `engage.jws` in the inputs, for establishing witnesses seated later.
    engagement_pool: Vec<Signed<Engagement>>,
    receipts: Receipts,
    /// Receipt bytes waiting for their engagement (embedded ones are verified in 3e).
    pending_receipts: Vec<Signed<Receipt>>,
    /// Decoded `keys/<kid>.revoked.jws` files, verified once their kid is established.
    revocation_pool: Vec<Signed<Revocation>>,
    /// Verified revocations by revoked `kid`.
    revocations: BTreeMap<String, Signed<Revocation>>,
    /// Every chain key ever established: `kid → (party, Grant exp)`.
    keys_seen: BTreeMap<String, (PartyIndex, u64)>,
    /// Nonces revealed so far, by party (§6.6).
    revealed: BTreeMap<PartyIndex, Vec<u8>>,
    verified: HashSet<Hash>,
    key_cache: BTreeMap<String, PublicKey>,
    state: Option<R::State>,
    committed: Vec<Committed>,
    recoveries: Vec<Recovery>,
    status: Status,
    anomalies: Vec<Anomaly>,
}

impl<'a, R: Rules> Fold<'a, R> {
    // ── Steps 1 and 2 ───────────────────────────────────────────────────────────────────────

    fn new(
        rules: &'a R,
        inputs: &Inputs,
        config: &Config,
        excluded: &HashSet<Hash>,
    ) -> Result<Self, Error> {
        let cap = config.max_body_cap;
        let mut seen = excluded.clone();
        let mut links: Vec<Signed<Link>> = decode_all(&inputs.links, typ::LINK, cap, &mut seen);
        let mut confirms: Vec<Signed<Confirmation>> =
            decode_all(&inputs.confirms, typ::CONFIRM, cap, &mut seen);
        let rejects: Vec<Signed<Reject>> = decode_all(&inputs.rejects, typ::REJECT, cap, &mut seen);
        let engagements: Vec<Signed<Engagement>> =
            decode_all(&inputs.engagements, typ::WITNESS, cap, &mut seen);
        let mut receipts: Vec<Signed<Receipt>> =
            decode_all(&inputs.receipts, typ::WITNESS, cap, &mut seen);
        let revocations: Vec<Signed<Revocation>> =
            decode_all(&inputs.revocations, typ::REVOKE, cap, &mut seen);

        // One party's mirrored links alone prove every committed link except the head (§9.1):
        // the confirmations and receipts embedded in links join the pools.
        let mut embedded_c = Vec::new();
        let mut embedded_r = Vec::new();
        for l in &links {
            for c in &l.payload.confirms {
                embedded_c.push(c.as_bytes().to_vec());
            }
            for r in &l.payload.receipts {
                embedded_r.push(r.as_bytes().to_vec());
            }
        }
        confirms.extend(decode_all(&embedded_c, typ::CONFIRM, cap, &mut seen));
        receipts.extend(decode_all(&embedded_r, typ::WITNESS, cap, &mut seen));

        // 1. Genesis.
        let mut anomalies = Vec::new();
        let mut geneses: Vec<(Signed<Link>, Genesis, String)> = Vec::new();
        for l in &links {
            if l.payload.seq != 0 || !l.payload.prev.is_empty() {
                continue;
            }
            let id = ChainId::derive(&l.bytes);
            if let Some(want) = &inputs.chain {
                if *want != id {
                    continue;
                }
            }
            if let Ok((g, path)) = validate_genesis(rules, l, config, inputs) {
                geneses.push((l.clone(), g, path));
            }
        }
        let (genesis_link, genesis, initiator_path) = match geneses.len() {
            0 => return Err(Error::NoChain("no valid genesis in the inputs".into())),
            1 => geneses.pop().expect("one"),
            _ => {
                return Err(Error::NoChain(
                    "several valid geneses; name the chain to fold".into(),
                ))
            }
        };
        let chain = ChainId::derive(&genesis_link.bytes);
        let n = genesis.parties.len();
        let q = genesis.confirm_quorum as usize;

        let mut pubkeys = Vec::with_capacity(n);
        for p in &genesis.parties {
            pubkeys.push(parse_z32(&p.pubky)?);
        }
        let mut key_cache = BTreeMap::new();
        let initiator_kid = genesis.parties[0].kid.clone().expect("checked by safety");
        let initiator_claims = pubky_common::auth::grant::GrantClaims::decode(
            genesis_link.payload.grant.as_deref().unwrap_or(""),
        )
        .map_err(|e| Error::Grant(format!("decode: {e}")))?;
        let mut seats: Vec<Option<Seat>> = vec![None; n];
        seats[0] = Some(Seat {
            pubky: genesis.parties[0].pubky.clone(),
            kid: initiator_kid.clone(),
            client_id: initiator_claims.client_id.to_string(),
            paths: vec![initiator_path],
            grant_exp: initiator_claims.exp,
            commit: None,
        });
        key_cache.insert(initiator_kid.clone(), parse_z32(&initiator_kid)?);

        // Keep only this chain's records. Genesis links other than ours are dropped.
        links.retain(|l| l.payload.chain == chain || l.hash == genesis_link.hash);
        confirms.retain(|c| c.payload.chain == chain);
        let rejects: Vec<_> = rejects
            .into_iter()
            .filter(|r| r.payload.chain == chain)
            .collect();

        // 2. Per-source, per-(seq, round), per-signer bound. Mirrors count against their
        //    signer, so a folder holding a whole QC is normal.
        {
            let mut counts: BTreeMap<(String, u64, u32, String), Vec<Hash>> = BTreeMap::new();
            let mut tally = |hash: Hash, seq: u64, round: u32, kid: &str| {
                if let Some(srcs) = inputs.sources.get(&hash) {
                    for s in srcs {
                        counts
                            .entry((s.clone(), seq, round, kid.to_string()))
                            .or_default()
                            .push(hash);
                    }
                }
            };
            for l in &links {
                tally(l.hash, l.payload.seq, l.payload.round, &l.payload.kid);
            }
            for c in &confirms {
                tally(
                    c.hash,
                    c.payload.seq,
                    c.payload.round.unwrap_or(0),
                    &c.payload.kid,
                );
            }
            for r in &rejects {
                tally(r.hash, r.payload.seq, r.payload.round, &r.payload.kid);
            }
            for ((_, seq, _, kid), hashes) in counts {
                if hashes.len() > config.records_per_signer_round {
                    anomalies.push(Anomaly {
                        against: Some(kid),
                        seq: Some(seq),
                        kind: AnomalyKind::HostileSource,
                        evidence: hashes,
                    });
                }
            }
        }

        let mut links_by_seq: BTreeMap<u64, Vec<Signed<Link>>> = BTreeMap::new();
        for l in links {
            links_by_seq.entry(l.payload.seq).or_default().push(l);
        }
        let mut confirms_by_seq: BTreeMap<u64, Vec<Signed<Confirmation>>> = BTreeMap::new();
        for c in confirms {
            confirms_by_seq.entry(c.payload.seq).or_default().push(c);
        }
        let mut rejects_by_seq: BTreeMap<u64, Vec<Signed<Reject>>> = BTreeMap::new();
        for r in rejects {
            rejects_by_seq.entry(r.payload.seq).or_default().push(r);
        }

        let mut fold = Fold {
            rules,
            chain,
            genesis,
            genesis_link,
            n,
            q,
            seats,
            pubkeys,
            keys_at: Vec::new(),
            links_by_seq,
            confirms_by_seq,
            rejects_by_seq,
            engaged: Vec::new(),
            engagement_keys: BTreeMap::new(),
            engagement_pool: engagements,
            receipts: Receipts::default(),
            pending_receipts: receipts,
            revocation_pool: revocations,
            revocations: BTreeMap::new(),
            keys_seen: BTreeMap::new(),
            revealed: BTreeMap::new(),
            verified: HashSet::new(),
            key_cache,
            state: None,
            committed: Vec::new(),
            recoveries: Vec::new(),
            status: Status::Ongoing,
            anomalies,
        };
        fold.keys_seen
            .insert(initiator_kid, (0, initiator_claims.exp));
        fold.establish_genesis_seats();
        for w in fold.genesis.witnesses.clone() {
            fold.establish_witness(&w.pubky);
        }
        fold.index_receipts();
        fold.index_revocations();
        Ok(fold)
    }

    /// Step 2: establish every party's kid, client_id and path from their genesis
    /// confirmation — inside the genesis QC or not (§6.6: key discovery as well as a vote).
    fn establish_genesis_seats(&mut self) {
        let genesis_hash = self.genesis_link.hash;
        let Some(confirms) = self.confirms_by_seq.get(&0).cloned() else {
            self.keys_at.push(self.current_keys());
            return;
        };
        for c in confirms {
            if c.payload.round != Some(0) || Hash::parse(&c.payload.link).ok() != Some(genesis_hash)
            {
                continue;
            }
            let (Some(grant), Some(path)) = (&c.payload.grant, &c.payload.path) else {
                continue;
            };
            let Ok(claims) = pubky_common::auth::grant::GrantClaims::decode(grant) else {
                continue;
            };
            let iss = claims.iss.z32();
            let Some(party) = self.genesis.parties.iter().position(|p| p.pubky == iss) else {
                continue;
            };
            if party == 0 || self.seats[party].is_some() {
                continue;
            }
            let Ok(kid_pk) = parse_z32(&c.payload.kid) else {
                continue;
            };
            if verify_grant(grant, &self.pubkeys[party], &kid_pk, path).is_err() {
                continue;
            }
            if c.verify(&kid_pk).is_err() {
                continue;
            }
            self.verified.insert(c.hash);
            self.key_cache.insert(c.payload.kid.clone(), kid_pk);
            self.keys_seen
                .insert(c.payload.kid.clone(), (party, claims.exp));
            self.seats[party] = Some(Seat {
                pubky: iss,
                kid: c.payload.kid.clone(),
                client_id: claims.client_id.to_string(),
                paths: vec![path.clone()],
                grant_exp: claims.exp,
                commit: c.payload.commit.clone(),
            });
        }
        self.keys_at.push(self.current_keys());
    }

    /// Verify one engagement as `pubky`'s: Grant under `pubky` with `cnf == kid`, write
    /// capability on its `path`, signature under `kid`.
    fn engagement_of(&mut self, e: &Signed<Engagement>, pubky: &str) -> Option<Engaged> {
        if e.payload.chain != self.chain || e.payload.kind != "engage" {
            return None;
        }
        let claims = pubky_common::auth::grant::GrantClaims::decode(&e.payload.grant).ok()?;
        if claims.iss.z32() != pubky {
            return None;
        }
        let pubky_pk = parse_z32(pubky).ok()?;
        let kid_pk = parse_z32(&e.payload.kid).ok()?;
        verify_grant(&e.payload.grant, &pubky_pk, &kid_pk, &e.payload.path).ok()?;
        e.verify(&kid_pk).ok()?;
        self.engagement_keys.insert(e.payload.kid.clone(), kid_pk);
        Some(Engaged {
            pubky: pubky.to_string(),
            kid: e.payload.kid.clone(),
            until: e.payload.until,
            poll_ms: e.payload.policy.poll_ms,
            path: e.payload.path.clone(),
        })
    }

    /// Step 2 / 3f: establish every engagement `pubky` has held for this chain from the pool
    /// and seat the governing one — the later `until` (§11.2). Genesis witnesses are
    /// established before the fold; witnesses seated by a `witnesses` link when it commits.
    fn establish_witness(&mut self, pubky: &str) {
        let pool = self.engagement_pool.clone();
        let mut mine: Vec<(Engaged, Hash)> = Vec::new();
        for e in &pool {
            if let Some(engaged) = self.engagement_of(e, pubky) {
                mine.push((engaged, e.hash));
            }
        }
        if mine.is_empty() {
            return;
        }
        let untils: BTreeSet<u64> = mine.iter().map(|(m, _)| m.until).collect();
        if untils.len() > 1 {
            self.anomalies.push(Anomaly {
                against: Some(pubky.to_string()),
                seq: None,
                kind: AnomalyKind::WitnessEquivocation,
                evidence: mine.iter().map(|(_, h)| *h).collect(),
            });
        }
        let (governing, _) = mine
            .into_iter()
            .max_by_key(|(m, h)| (m.until, *h))
            .expect("non-empty");
        self.engaged.retain(|w| w.pubky != pubky);
        self.engaged.push(governing);
    }

    /// Index every receipt whose engagement is established, verified under that key.
    fn index_receipts(&mut self) {
        let pending = std::mem::take(&mut self.pending_receipts);
        for r in pending {
            if r.payload.chain != self.chain || r.payload.kind != "observed" {
                continue;
            }
            match self.engagement_keys.get(&r.payload.kid) {
                Some(pk) if r.verify(pk).is_ok() => {
                    let _ = self.receipts.insert(r);
                }
                _ => self.pending_receipts.push(r),
            }
        }
    }

    /// Step 2: revocations, as discovery only. One per established kid; verified under the
    /// embedded Grant of the same pubky (§5.3), signed by that Grant's `cnf`.
    fn index_revocations(&mut self) {
        let pending = std::mem::take(&mut self.revocation_pool);
        for r in pending {
            if r.payload.kind != "revoked" || self.revocations.contains_key(&r.payload.revoked) {
                continue;
            }
            let Some(&(party, _)) = self.keys_seen.get(&r.payload.revoked) else {
                self.revocation_pool.push(r);
                continue;
            };
            let Ok(claims) = pubky_common::auth::grant::GrantClaims::decode(&r.payload.grant)
            else {
                continue;
            };
            let Some(seat) = self.seats[party].as_ref() else {
                continue;
            };
            if claims.iss.z32() != seat.pubky || claims.cnf.z32() != r.payload.by {
                continue;
            }
            let Ok(by_pk) = parse_z32(&r.payload.by) else {
                continue;
            };
            let path = seat.paths.last().cloned().unwrap_or_default();
            if verify_grant(&r.payload.grant, &self.pubkeys[party], &by_pk, &path).is_err()
                || r.verify(&by_pk).is_err()
            {
                continue;
            }
            self.revocations.insert(r.payload.revoked.clone(), r);
        }
    }

    // ── Helpers ─────────────────────────────────────────────────────────────────────────────

    fn current_keys(&self) -> BTreeMap<String, PartyIndex> {
        self.seats
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|s| (s.kid.clone(), i)))
            .collect()
    }

    fn party_by_pubky(&self, pubky: &str) -> Option<PartyIndex> {
        self.genesis.parties.iter().position(|p| p.pubky == pubky)
    }

    fn key(&mut self, kid: &str) -> Option<PublicKey> {
        if let Some(k) = self.key_cache.get(kid) {
            return Some(k.clone());
        }
        let k = parse_z32(kid).ok()?;
        self.key_cache.insert(kid.to_string(), k.clone());
        Some(k)
    }

    /// Verify a record's signature under `kid`, once.
    fn sig_ok<T: serde::de::DeserializeOwned>(&mut self, s: &Signed<T>, kid: &str) -> bool {
        if self.verified.contains(&s.hash) {
            return true;
        }
        let Some(pk) = self.key(kid) else {
            return false;
        };
        if s.verify(&pk).is_ok() {
            self.verified.insert(s.hash);
            true
        } else {
            false
        }
    }

    fn keys_for(&self, seq: u64) -> &BTreeMap<String, PartyIndex> {
        let i = (seq as usize).min(self.keys_at.len() - 1);
        &self.keys_at[i]
    }

    fn seat_kid(&self, party: PartyIndex) -> Option<String> {
        self.seats[party].as_ref().map(|s| s.kid.clone())
    }

    fn anomaly(&mut self, against: Option<&str>, seq: u64, kind: AnomalyKind, evidence: Vec<Hash>) {
        self.anomalies.push(Anomaly {
            against: against.map(str::to_string),
            seq: Some(seq),
            kind,
            evidence,
        });
    }

    /// Step 3e: decode and verify a link's embedded `confirms` as a QC for `prev`, under the
    /// keys valid at `prev`'s seq. `None` if it is not one.
    fn embedded_qc(
        &mut self,
        confirms: &[String],
        prev: &Committed,
        keys: &BTreeMap<String, PartyIndex>,
    ) -> Option<Vec<Signed<Confirmation>>> {
        let quorum = self.quorum_of(&prev.link.payload);
        let mut out = Vec::new();
        let mut voters: BTreeSet<PartyIndex> = BTreeSet::new();
        voters.insert(prev.author);
        for c in confirms {
            let s = decode_confirmation(c.as_bytes().to_vec()).ok()?;
            if s.payload.chain != self.chain
                || s.payload.seq != prev.link.payload.seq
                || s.payload.round != Some(prev.link.payload.round)
                || Hash::parse(&s.payload.link).ok()? != prev.link.hash
            {
                return None;
            }
            let party = *keys.get(&s.payload.kid)?;
            if !self.sig_ok(&s, &s.payload.kid) {
                return None;
            }
            voters.insert(party);
            out.push(s);
        }
        (voters.len() >= quorum).then_some(out)
    }

    /// The quorum a committed link needed.
    fn quorum_of(&self, link: &Link) -> usize {
        effective_quorum(link.kind(), self.n, self.q)
    }

    /// The rules' allowances (§11.3), defaults applied.
    fn allowances(&self) -> (u64, u64) {
        let tc = self.genesis.time_control();
        (tc.think_ms, tc.respond_ms)
    }

    // ── Step 3 ──────────────────────────────────────────────────────────────────────────────

    fn run(&mut self) -> Result<(), Error> {
        let mut seq: u64 = 0;
        loop {
            let candidates = if seq == 0 {
                vec![Candidate {
                    link: self.genesis_link.clone(),
                    author: 0,
                    kind: Kind::Rules,
                    next_state: None,
                    quorum: self.q,
                    key_change: None,
                    reveal: None,
                    witnesses: None,
                    embedded_qc: Vec::new(),
                }]
            } else {
                self.candidates(seq)
            };
            let mut view = SeqView {
                candidates,
                rounds: BTreeMap::new(),
                present: BTreeSet::new(),
                records: BTreeMap::new(),
                vetoes: BTreeMap::new(),
                skips: Vec::new(),
            };
            self.votes(seq, &mut view);
            self.unjustified_rounds(seq, &view);
            self.premature_skips(seq, &view);

            loop {
                match self.decide(seq, &view)? {
                    Decision::History { index, qc } => {
                        let cand = view.candidates[index].clone();
                        self.report_late_votes(seq, cand.link.hash, &view);
                        if cand.kind == Kind::Recover {
                            // History: a veto or a delay verdict surfacing now changes nothing.
                            if let Some(v) = view.vetoes.get(&cand.author) {
                                let old = self.seat_kid(cand.author);
                                self.anomaly(old.as_deref(), seq, AnomalyKind::LateVeto, v.clone());
                            }
                            let delay = self.recover_delay(&cand, &qc);
                            if delay == TimeVerdict::No {
                                self.premature_recover(seq, &cand, &qc);
                            }
                            self.recoveries.push(Recovery {
                                seq,
                                party: cand.author,
                                new_kid: cand.link.payload.kid.clone(),
                                link: cand.link.hash,
                                delay,
                            });
                        }
                        if self.commit(seq, &cand, qc)? {
                            return Ok(());
                        }
                        break;
                    }
                    Decision::Head { index, qc } => {
                        let cand = view.candidates[index].clone();
                        if cand.kind == Kind::Recover {
                            // Provisional: the old key's veto voids it, whatever votes it has.
                            if let Some(v) = view.vetoes.get(&cand.author).cloned() {
                                let mut ev = vec![cand.link.hash];
                                ev.extend(v);
                                self.anomaly(
                                    Some(&cand.link.payload.kid),
                                    seq,
                                    AnomalyKind::VetoedRecover,
                                    ev,
                                );
                                view.candidates.remove(index);
                                continue;
                            }
                            // Delay, by the witness quorum: invalid only on a quorum's `No`.
                            let delay = self.recover_delay(&cand, &qc);
                            if delay == TimeVerdict::No {
                                self.premature_recover(seq, &cand, &qc);
                                view.candidates.remove(index);
                                continue;
                            }
                            self.recoveries.push(Recovery {
                                seq,
                                party: cand.author,
                                new_kid: cand.link.payload.kid.clone(),
                                link: cand.link.hash,
                                delay,
                            });
                        }
                        self.report_late_votes(seq, cand.link.hash, &view);
                        if self.commit(seq, &cand, qc)? {
                            return Ok(());
                        }
                        break;
                    }
                    Decision::None => {
                        if seq > 0 {
                            match self.abandoned_close(seq, &view)? {
                                CloseOutcome::Ended => return Ok(()),
                                CloseOutcome::Paused(status) => {
                                    self.status = status;
                                    return Ok(());
                                }
                                CloseOutcome::NoClose => {}
                            }
                        }
                        self.stall(seq, &view);
                        return Ok(());
                    }
                }
            }
            seq += 1;
        }
    }

    /// Step 3a: candidates at `seq`.
    fn candidates(&mut self, seq: u64) -> Vec<Candidate<R::State>> {
        let prev = self.committed.last().expect("seq > 0 has a head").clone();
        let prev_keys = self.keys_for(prev.link.payload.seq).clone();
        let keys = self.keys_for(seq).clone();
        let links = self.links_by_seq.get(&seq).cloned().unwrap_or_default();
        let mut out = Vec::new();
        for l in links {
            if l.payload.v != crate::PROTOCOL_VERSION
                || l.payload.chain != self.chain
                || l.payload.prev_hash().ok().flatten() != Some(prev.link.hash)
            {
                continue;
            }
            let kind = l.payload.kind();
            // The signer: the seat's established key — except a `recover`, signed by the new
            // key its embedded Grant binds (§6.7).
            let (author, key_change) = if kind == Kind::Recover {
                let Some(author) = self.party_by_pubky(&l.payload.author) else {
                    continue;
                };
                if self.seats[author].is_none() {
                    continue;
                }
                if l.payload.state != prev.link.payload.state {
                    continue;
                }
                let Ok(body) = serde_json::from_value::<KeyChangeBody>(l.payload.body.clone())
                else {
                    continue;
                };
                let (Some(grant), Some(path)) = (&l.payload.grant, &body.path) else {
                    continue;
                };
                if body.new_kid != l.payload.kid || keys.contains_key(&body.new_kid) {
                    continue;
                }
                let Ok(new_pk) = parse_z32(&body.new_kid) else {
                    continue;
                };
                let Ok(claims) = verify_grant(grant, &self.pubkeys[author], &new_pk, path) else {
                    continue;
                };
                (
                    author,
                    Some(KeyChange {
                        kid: body.new_kid.clone(),
                        exp: claims.exp,
                        client_id: Some(claims.client_id.to_string()),
                        path: Some(path.clone()),
                    }),
                )
            } else {
                let Some(&author) = keys.get(&l.payload.kid) else {
                    continue;
                };
                if self.genesis.parties[author].pubky != l.payload.author {
                    continue;
                }
                (author, None)
            };
            if !self.sig_ok(&l, &l.payload.kid) {
                continue;
            }
            let Some(embedded) = self.embedded_qc(&l.payload.confirms, &prev, &prev_keys) else {
                continue;
            };
            // Abandoned closes are judged outside rounds (step h).
            let is_abandoned = kind == Kind::Close
                && serde_json::from_value::<CloseBody>(l.payload.body.clone())
                    .map(|b| b.reason == CloseReason::Abandoned)
                    .unwrap_or(false);
            if is_abandoned {
                continue;
            }
            // Round >= 1: the designated party, except reveal (fixed author).
            if l.payload.round >= 1
                && kind != Kind::Reveal
                && designated(&self.chain, seq, l.payload.round, self.n) != author
            {
                continue;
            }
            let mut cand = Candidate {
                link: l.clone(),
                author,
                kind,
                next_state: None,
                quorum: effective_quorum(kind, self.n, self.q),
                key_change,
                reveal: None,
                witnesses: None,
                embedded_qc: embedded,
            };
            match kind {
                Kind::Rules => {
                    let Some(state) = self.state.as_ref() else {
                        continue;
                    };
                    if !self.rules.may_append(state, author, &l.payload.kind) {
                        continue;
                    }
                    let Ok(next) = self.rules.apply(state, &l.payload) else {
                        continue;
                    };
                    if Hash::parse(&l.payload.state).ok() != Some(state_hash(self.rules, &next)) {
                        continue;
                    }
                    cand.next_state = Some(next);
                }
                Kind::Close => {
                    if l.payload.state != prev.link.payload.state {
                        continue;
                    }
                    let Ok(body) = serde_json::from_value::<CloseBody>(l.payload.body.clone())
                    else {
                        continue;
                    };
                    let Some(state) = self.state.as_ref() else {
                        continue;
                    };
                    if body.reason == CloseReason::Finished
                        && !matches!(self.rules.status(state), RulesStatus::Finished(_))
                    {
                        continue;
                    }
                    if self.rules.close(state, &body).is_err() {
                        continue;
                    }
                }
                Kind::Rekey => {
                    if l.payload.state != prev.link.payload.state {
                        continue;
                    }
                    let Ok(body) = serde_json::from_value::<KeyChangeBody>(l.payload.body.clone())
                    else {
                        continue;
                    };
                    let Some(grant) = &l.payload.grant else {
                        continue;
                    };
                    let seat = self.seats[author].clone().expect("author is seated");
                    let Ok(new_pk) = parse_z32(&body.new_kid) else {
                        continue;
                    };
                    let path = seat.paths.last().cloned().unwrap_or_default();
                    let Ok(claims) = verify_grant(grant, &self.pubkeys[author], &new_pk, &path)
                    else {
                        continue;
                    };
                    if claims.client_id.to_string() != seat.client_id {
                        continue;
                    }
                    cand.key_change = Some(KeyChange {
                        kid: body.new_kid,
                        exp: claims.exp,
                        client_id: None,
                        path: None,
                    });
                }
                Kind::Recover => {}
                Kind::Reveal => {
                    // The next party in genesis order who has not revealed; nobody else. The
                    // initiator's contribution is `genesis.nonce`, so reveals start at 1.
                    if !self.rules.wants_reveals(&self.genesis)
                        || l.payload.state != prev.link.payload.state
                    {
                        continue;
                    }
                    let next = (1..self.n).find(|p| !self.revealed.contains_key(p));
                    if next != Some(author) {
                        continue;
                    }
                    let Ok(body) = serde_json::from_value::<RevealBody>(l.payload.body.clone())
                    else {
                        continue;
                    };
                    let Ok(nonce) =
                        base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&body.nonce)
                    else {
                        continue;
                    };
                    let commit = self.seats[author].as_ref().and_then(|s| s.commit.clone());
                    let Some(commit) = commit.and_then(|c| Hash::parse(&c).ok()) else {
                        continue;
                    };
                    if Hash::of(&nonce) != commit {
                        continue;
                    }
                    cand.reveal = Some(nonce);
                }
                Kind::Witnesses => {
                    if l.payload.state != prev.link.payload.state {
                        continue;
                    }
                    let Ok(body) = serde_json::from_value::<WitnessChange>(l.payload.body.clone())
                    else {
                        continue;
                    };
                    if body.add.is_empty() && body.remove.is_empty() {
                        continue;
                    }
                    let mut adds = Vec::new();
                    let mut ok = true;
                    for add in &body.add {
                        let Ok(e) = decode_engagement(add.engage.as_bytes().to_vec()) else {
                            ok = false;
                            break;
                        };
                        if e.payload.kid != add.kid {
                            ok = false;
                            break;
                        }
                        match self.engagement_of(&e, &add.pubky) {
                            Some(engaged) => adds.push((engaged, e)),
                            None => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if !ok {
                        continue;
                    }
                    cand.witnesses = Some((adds, body.remove.clone()));
                }
            }
            out.push(cand);
        }
        out
    }

    /// Steps 3b–3c: votes per `(party, round)`, vetoes set aside, presence and records kept
    /// for steps g and h.
    fn votes(&mut self, seq: u64, view: &mut SeqView<R::State>) {
        let keys = self.keys_for(seq).clone();
        let mut evidence: BTreeMap<(u32, PartyIndex), Vec<Hash>> = BTreeMap::new();
        let recovering: BTreeSet<PartyIndex> = view
            .candidates
            .iter()
            .filter(|c| c.kind == Kind::Recover)
            .map(|c| c.author)
            .collect();
        for c in &view.candidates {
            view.rounds
                .entry(c.link.payload.round)
                .or_default()
                .propose(c.author, c.link.hash);
            evidence
                .entry((c.link.payload.round, c.author))
                .or_default()
                .push(c.link.hash);
            view.present.insert(c.author);
            view.records.entry(c.author).or_default().push(c.link.hash);
        }
        let by_hash: BTreeMap<Hash, (String, u32)> = view
            .candidates
            .iter()
            .map(|c| {
                (
                    c.link.hash,
                    (c.link.payload.state.clone(), c.link.payload.round),
                )
            })
            .collect();

        for c in self.confirms_by_seq.get(&seq).cloned().unwrap_or_default() {
            let Some(&party) = keys.get(&c.payload.kid) else {
                continue;
            };
            if !self.sig_ok(&c, &c.payload.kid) {
                continue;
            }
            let Ok(link) = Hash::parse(&c.payload.link) else {
                continue;
            };
            view.present.insert(party);
            view.records.entry(party).or_default().push(c.hash);
            let Some(round) = c.payload.round else {
                continue; // an abandoned-close confirmation: step h
            };
            if seq == 0 && link != self.genesis_link.hash {
                continue;
            }
            view.rounds
                .entry(round)
                .or_default()
                .cast(party, Vote::For(link));
            evidence.entry((round, party)).or_default().push(c.hash);
            if let Some((state, _)) = by_hash.get(&link) {
                if seq > 0 && *state != c.payload.state {
                    self.anomaly(
                        Some(&c.payload.kid),
                        seq,
                        AnomalyKind::RulesDivergence,
                        vec![link, c.hash],
                    );
                }
            }
        }

        // Rounds in which the designated party acted (proposed or passed): an empty reject by
        // anyone else there is an ordinary vote for nothing, not a skip (§6.3).
        let rejects = self.rejects_by_seq.get(&seq).cloned().unwrap_or_default();
        let mut designated_acted: BTreeSet<u32> = view
            .candidates
            .iter()
            .filter(|c| {
                c.link.payload.round >= 1
                    && designated(&self.chain, seq, c.link.payload.round, self.n) == c.author
            })
            .map(|c| c.link.payload.round)
            .collect();
        for r in &rejects {
            if r.payload.round >= 1 && r.payload.is_empty() {
                if let Some(&party) = keys.get(&r.payload.kid) {
                    if designated(&self.chain, seq, r.payload.round, self.n) == party
                        && self.sig_ok(r, &r.payload.kid)
                    {
                        designated_acted.insert(r.payload.round);
                    }
                }
            }
        }

        let mut skipped: BTreeSet<PartyIndex> = BTreeSet::new();
        for r in rejects {
            let Some(&party) = keys.get(&r.payload.kid) else {
                continue;
            };
            if !self.sig_ok(&r, &r.payload.kid) {
                continue;
            }
            view.present.insert(party);
            view.records.entry(party).or_default().push(r.hash);
            // An established-key reject at a seq holding a recover for that seat is a veto,
            // not a round vote (§6.3, §6.7).
            if recovering.contains(&party) {
                view.vetoes.entry(party).or_default().push(r.hash);
                continue;
            }
            let round = r.payload.round;
            let proposals_in_round: Vec<Hash> = view
                .candidates
                .iter()
                .filter(|c| c.link.payload.round == round)
                .map(|c| c.link.hash)
                .collect();
            if r.payload.is_empty() {
                let is_pass = round >= 1 && designated(&self.chain, seq, round, self.n) == party;
                let is_plain = round >= 1 && designated_acted.contains(&round);
                if !is_pass && !is_plain {
                    if round == 0 || !skipped.insert(party) {
                        self.anomaly(
                            Some(&r.payload.kid),
                            seq,
                            AnomalyKind::InvalidSkip,
                            vec![r.hash],
                        );
                        continue;
                    }
                    view.skips.push((party, round, r.hash));
                } else if is_plain && proposals_in_round.len() == 1 {
                    // A vote for nothing against the sole valid proposal (§6.3).
                    self.anomaly(
                        Some(&r.payload.kid),
                        seq,
                        AnomalyKind::Obstruction,
                        vec![proposals_in_round[0], r.hash],
                    );
                }
            } else if let Ok(target) = Hash::parse(&r.payload.link) {
                if by_hash.contains_key(&target) && proposals_in_round.len() == 1 {
                    self.anomaly(
                        Some(&r.payload.kid),
                        seq,
                        AnomalyKind::Obstruction,
                        vec![target, r.hash],
                    );
                }
            }
            view.rounds
                .entry(round)
                .or_default()
                .cast(party, Vote::Nothing);
            evidence.entry((round, party)).or_default().push(r.hash);
        }

        let equivocators: Vec<(u32, PartyIndex)> = view
            .rounds
            .iter()
            .flat_map(|(round, votes)| votes.equivocators().into_iter().map(move |p| (*round, p)))
            .collect();
        for (round, party) in equivocators {
            let kid = self.seat_kid(party);
            let ev = evidence.get(&(round, party)).cloned().unwrap_or_default();
            self.anomaly(kid.as_deref(), seq, AnomalyKind::Equivocation, ev);
        }
    }

    /// Step 3g: a round `r > 0` whose predecessor is not dead in the files.
    fn unjustified_rounds(&mut self, seq: u64, view: &SeqView<R::State>) {
        for c in &view.candidates {
            let r = c.link.payload.round;
            if r == 0 {
                continue;
            }
            let dead = view
                .rounds
                .get(&(r - 1))
                .map(|v| v.is_dead(self.n, self.q))
                .unwrap_or(false);
            if !dead {
                self.anomaly(
                    Some(&c.link.payload.kid),
                    seq,
                    AnomalyKind::UnjustifiedRound,
                    vec![c.link.hash],
                );
            }
        }
    }

    // ── The stopwatch (§11.3) ───────────────────────────────────────────────────────────────

    // Every judgement below is made only from a witness's COMPLETE view of the records the
    // files hold: a witness missing a receipt for any record that bears on the question
    // cannot judge (`None`), never "did not see it". That is what keeps a verifier with fewer
    // receipts at *asserted* rather than at the opposite final (§6.8, §11.2).

    /// `ready(seq)` as witness `kid` saw it: the latest QC confirmation of the head. `None`
    /// unless it receipted every one.
    fn ready_as_seen_by(&self, kid: &str) -> Option<u64> {
        let head = self.committed.last()?;
        let mut latest = None;
        for c in &head.qc {
            let t = self.receipts.observed(&c.hash, kid)?;
            latest = Some(latest.map_or(t, |l: u64| l.max(t)));
        }
        latest
    }

    /// The latest record by `party` at this seq that witness `kid` observed at or before `at`.
    /// `Ok(None)` if the party has none; `Err(())` if the witness missed one of them.
    fn last_record_before(
        &self,
        view: &SeqView<R::State>,
        party: PartyIndex,
        kid: &str,
        at: u64,
    ) -> Result<Option<u64>, ()> {
        let Some(records) = view.records.get(&party) else {
            return Ok(None);
        };
        let mut latest = None;
        for h in records {
            let t = self.receipts.observed(h, kid).ok_or(())?;
            if t <= at {
                latest = Some(latest.map_or(t, |l: u64| l.max(t)));
            }
        }
        Ok(latest)
    }

    /// Step 3g, with receipts: a skip observed before the designated party had been silent for
    /// `think_ms` is premature — an anomaly against the skipper, never a validity question.
    fn premature_skips(&mut self, seq: u64, view: &SeqView<R::State>) {
        if self.engaged.is_empty() || view.skips.is_empty() {
            return;
        }
        let (think_ms, _) = self.allowances();
        let engaged = self.engaged.clone();
        for (skipper, round, skip) in &view.skips {
            let skipped = designated(&self.chain, seq, *round, self.n);
            let judgements: Vec<Option<bool>> = engaged
                .iter()
                .map(|w| {
                    let t_skip = self.receipts.observed(skip, &w.kid)?;
                    let mut start = self.ready_as_seen_by(&w.kid)?;
                    if let Some(t) = self
                        .last_record_before(view, skipped, &w.kid, t_skip)
                        .ok()?
                    {
                        start = start.max(t);
                    }
                    Some(crate::witness::stopwatch::silence(t_skip, start) < think_ms)
                })
                .collect();
            if adjudicate(&judgements) == TimeVerdict::Yes {
                let kid = self.seat_kid(*skipper);
                self.anomaly(kid.as_deref(), seq, AnomalyKind::PrematureSkip, vec![*skip]);
            }
        }
    }

    /// Step 3f: was `recovery_delay_ms` honoured between the recover and its earliest
    /// confirmation, by the witness quorum? A witness judges only if it receipted the recover
    /// and every confirmation of its QC.
    fn recover_delay(
        &self,
        cand: &Candidate<R::State>,
        qc: &[Signed<Confirmation>],
    ) -> TimeVerdict {
        if self.engaged.is_empty() {
            return TimeVerdict::Asserted {
                yes: 0,
                no: 0,
                silent: 0,
            };
        }
        let delay = self.genesis.recovery_delay_ms;
        let judgements: Vec<Option<bool>> = self
            .engaged
            .iter()
            .map(|w| {
                let t_rec = self.receipts.observed(&cand.link.hash, &w.kid)?;
                let mut first: Option<u64> = None;
                for c in qc {
                    let t = self.receipts.observed(&c.hash, &w.kid)?;
                    first = Some(first.map_or(t, |f| f.min(t)));
                }
                let first = first?;
                Some(first.saturating_sub(t_rec) >= delay)
            })
            .collect();
        adjudicate(&judgements)
    }

    /// The confirmers of a recover the quorum found premature.
    fn premature_recover(
        &mut self,
        seq: u64,
        cand: &Candidate<R::State>,
        qc: &[Signed<Confirmation>],
    ) {
        for c in qc {
            self.anomaly(
                Some(&c.payload.kid),
                seq,
                AnomalyKind::PrematureRecover,
                vec![cand.link.hash, c.hash],
            );
        }
    }

    // ── Step 3d ─────────────────────────────────────────────────────────────────────────────

    fn decide(&mut self, seq: u64, view: &SeqView<R::State>) -> Result<Decision, Error> {
        let candidates = &view.candidates;
        // A successor with a QC embeds this seq's QC: history.
        let mut history: Option<(usize, Vec<Signed<Confirmation>>)> = None;
        let successors = self
            .links_by_seq
            .get(&(seq + 1))
            .cloned()
            .unwrap_or_default();
        for s in successors {
            if s.payload.chain != self.chain {
                continue;
            }
            let Ok(Some(prev_hash)) = s.payload.prev_hash() else {
                continue;
            };
            let Some(index) = candidates.iter().position(|c| c.link.hash == prev_hash) else {
                continue;
            };
            let cand = &candidates[index];
            let keys_now = self.keys_for(seq).clone();
            let keys_after = self.keys_after(cand);
            // The successor's signer: a seated key, or — for a recover — its own new key.
            let s_author = if s.payload.kind() == Kind::Recover {
                self.party_by_pubky(&s.payload.author)
            } else {
                keys_after.get(&s.payload.kid).copied()
            };
            let Some(s_author) = s_author else {
                continue;
            };
            if self.genesis.parties[s_author].pubky != s.payload.author
                || !self.sig_ok(&s, &s.payload.kid)
            {
                continue;
            }
            let as_committed = Committed {
                link: cand.link.clone(),
                author: cand.author,
                qc: Vec::new(),
                is_final: false,
                witnessed: (0, 0),
            };
            let Some(qc) = self.embedded_qc(&s.payload.confirms, &as_committed, &keys_now) else {
                continue;
            };
            if !self.has_qc(&s, s_author, &keys_after) {
                continue;
            }
            match &history {
                Some((i, _)) if *i != index => {
                    return Err(Error::CollectiveEquivocation {
                        seq,
                        a: candidates[*i].link.hash,
                        b: cand.link.hash,
                    })
                }
                Some(_) => {}
                None => history = Some((index, qc)),
            }
        }
        if let Some((index, qc)) = history {
            return Ok(Decision::History { index, qc });
        }

        // Otherwise the QC in the highest round. QCs only — never death evidence.
        for (round, votes) in view.rounds.iter().rev() {
            let mut qcs: Vec<usize> = Vec::new();
            for (i, c) in candidates.iter().enumerate() {
                if c.link.payload.round == *round && votes.votes_for(&c.link.hash) >= c.quorum {
                    qcs.push(i);
                }
            }
            match qcs.len() {
                0 => continue,
                1 => {
                    let index = qcs[0];
                    let qc = self.qc_confirmations(seq, &candidates[index]);
                    return Ok(Decision::Head { index, qc });
                }
                _ => {
                    return Err(Error::CollectiveEquivocation {
                        seq,
                        a: candidates[qcs[0]].link.hash,
                        b: candidates[qcs[1]].link.hash,
                    })
                }
            }
        }
        Ok(Decision::None)
    }

    /// The key map after `cand` commits (a `rekey` or `recover` changes its author's key).
    fn keys_after(&self, cand: &Candidate<R::State>) -> BTreeMap<String, PartyIndex> {
        let mut keys = self.current_keys();
        if let Some(kc) = &cand.key_change {
            keys.retain(|_, p| *p != cand.author);
            keys.insert(kc.kid.clone(), cand.author);
        }
        keys
    }

    /// Does `link` (at its own seq) hold a QC in the files, under `keys`?
    fn has_qc(
        &mut self,
        link: &Signed<Link>,
        author: PartyIndex,
        keys: &BTreeMap<String, PartyIndex>,
    ) -> bool {
        let quorum = self.quorum_of(&link.payload);
        let mut voters = BTreeSet::new();
        voters.insert(author);
        for c in self
            .confirms_by_seq
            .get(&link.payload.seq)
            .cloned()
            .unwrap_or_default()
        {
            if c.payload.round != Some(link.payload.round)
                || Hash::parse(&c.payload.link).ok() != Some(link.hash)
            {
                continue;
            }
            let Some(&p) = keys.get(&c.payload.kid) else {
                continue;
            };
            if self.sig_ok(&c, &c.payload.kid) {
                voters.insert(p);
            }
        }
        voters.len() >= quorum
    }

    /// The confirmations in the files forming `cand`'s QC in its round: one per party, the
    /// lowest hash if a party gave several.
    fn qc_confirmations(
        &mut self,
        seq: u64,
        cand: &Candidate<R::State>,
    ) -> Vec<Signed<Confirmation>> {
        let keys = self.keys_for(seq).clone();
        let mut per_party: BTreeMap<PartyIndex, Signed<Confirmation>> = BTreeMap::new();
        for c in self.confirms_by_seq.get(&seq).cloned().unwrap_or_default() {
            if c.payload.round != Some(cand.link.payload.round)
                || Hash::parse(&c.payload.link).ok() != Some(cand.link.hash)
            {
                continue;
            }
            let Some(&p) = keys.get(&c.payload.kid) else {
                continue;
            };
            if p == cand.author || !self.sig_ok(&c, &c.payload.kid) {
                continue;
            }
            match per_party.get(&p) {
                Some(existing) if existing.hash <= c.hash => {}
                _ => {
                    per_party.insert(p, c);
                }
            }
        }
        let mut qc: Vec<_> = per_party.into_values().collect();
        qc.sort_by(|a, b| a.payload.kid.cmp(&b.payload.kid));
        qc
    }

    /// Any other QC at this seq is a late vote, never an input.
    fn report_late_votes(&mut self, seq: u64, committed: Hash, view: &SeqView<R::State>) {
        for c in &view.candidates {
            if c.link.hash == committed {
                continue;
            }
            let Some(votes) = view.rounds.get(&c.link.payload.round) else {
                continue;
            };
            if votes.votes_for(&c.link.hash) >= c.quorum {
                self.anomaly(None, seq, AnomalyKind::LateVote, vec![c.link.hash]);
            }
        }
    }

    /// Steps 3e–3f on the committed link. Returns `true` if the chain ended here.
    fn commit(
        &mut self,
        seq: u64,
        cand: &Candidate<R::State>,
        qc: Vec<Signed<Confirmation>>,
    ) -> Result<bool, Error> {
        // 3e: each receipt PRESENT in the link's `receipts` is for a member of its embedded QC
        //     and verifies under an engagement established at or before seq − 1.
        if seq > 0 {
            let members: BTreeSet<Hash> = cand.embedded_qc.iter().map(|c| c.hash).collect();
            for r in &cand.link.payload.receipts {
                let ok = decode_receipt(r.as_bytes().to_vec()).ok().and_then(|s| {
                    let pk = self.engagement_keys.get(&s.payload.kid)?;
                    let record = Hash::parse(&s.payload.record).ok()?;
                    (s.payload.chain == self.chain
                        && members.contains(&record)
                        && s.verify(pk).is_ok())
                    .then_some(s)
                });
                match ok {
                    Some(s) => {
                        let _ = self.receipts.insert(s);
                    }
                    None => self.anomaly(
                        Some(&cand.link.payload.kid),
                        seq,
                        AnomalyKind::BadEmbeddedReceipt,
                        vec![cand.link.hash],
                    ),
                }
            }
        }

        let witnessed = self.witnessed(&qc);
        if let Some(prev) = self.committed.last_mut() {
            prev.is_final = true;
        }
        self.committed.push(Committed {
            link: cand.link.clone(),
            author: cand.author,
            qc: qc.clone(),
            is_final: false,
            witnessed,
        });

        // 3f.
        let mut ended = false;
        if seq == 0 {
            if !self.rules.wants_reveals(&self.genesis) {
                let confirmations: Vec<Confirmation> =
                    qc.iter().map(|c| c.payload.clone()).collect();
                let state = self
                    .rules
                    .init(&self.genesis, &confirmations, &[])
                    .map_err(|e| Error::Rules(e.0))?;
                self.state = Some(state);
            }
        } else {
            match cand.kind {
                Kind::Rules => {
                    self.state = cand.next_state.clone();
                }
                Kind::Close => {
                    let body: CloseBody = serde_json::from_value(cand.link.payload.body.clone())
                        .expect("checked in 3a");
                    let state = self.state.as_ref().expect("checked in 3a");
                    let outcome = self
                        .rules
                        .close(state, &body)
                        .map_err(|e| Error::Rules(e.0))?;
                    self.status = Status::Closed(outcome);
                    ended = true;
                }
                Kind::Rekey | Kind::Recover => {
                    let kc = cand.key_change.clone().expect("checked in 3a");
                    self.keys_seen.insert(kc.kid.clone(), (cand.author, kc.exp));
                    let seat = self.seats[cand.author].as_mut().expect("seated");
                    seat.kid = kc.kid;
                    seat.grant_exp = kc.exp;
                    if let Some(client_id) = kc.client_id {
                        seat.client_id = client_id;
                    }
                    if let Some(path) = kc.path {
                        if seat.paths.last() != Some(&path) {
                            seat.paths.push(path);
                        }
                    }
                }
                Kind::Reveal => {
                    let nonce = cand.reveal.clone().expect("checked in 3a");
                    self.revealed.insert(cand.author, nonce);
                    if (1..self.n).all(|p| self.revealed.contains_key(&p)) {
                        // The last reveal: seats are drawn and the rules initialise (§6.6).
                        let genesis_nonce = base64::engine::general_purpose::URL_SAFE_NO_PAD
                            .decode(&self.genesis.nonce)
                            .map_err(|e| Error::Genesis(format!("nonce: {e}")))?;
                        let mut nonces = vec![genesis_nonce];
                        for p in 1..self.n {
                            nonces.push(self.revealed[&p].clone());
                        }
                        let confirmations: Vec<Confirmation> = self.committed[0]
                            .qc
                            .iter()
                            .map(|c| c.payload.clone())
                            .collect();
                        let state = self
                            .rules
                            .init(&self.genesis, &confirmations, &nonces)
                            .map_err(|e| Error::Rules(e.0))?;
                        self.state = Some(state);
                    }
                }
                Kind::Witnesses => {
                    // Every party confirmed it (quorum N in 3b). Seat the added witnesses —
                    // their keys are established here, so their receipts become usable — and
                    // unseat the removed kids, both from the next seq.
                    let (adds, removes) = cand.witnesses.clone().expect("checked in 3a");
                    for (engaged, e) in adds {
                        self.engagement_pool.push(e);
                        self.engaged.retain(|w| w.pubky != engaged.pubky);
                        self.engaged.push(engaged.clone());
                        // Later rotations of this witness are found from the pool from here.
                        self.establish_witness(&engaged.pubky);
                    }
                    self.engaged.retain(|w| !removes.contains(&w.kid));
                }
            }
        }
        self.keys_at.push(self.current_keys());
        self.index_receipts();
        self.index_revocations();
        Ok(ended)
    }

    /// *Witnessed m/k* for a QC.
    fn witnessed(&self, qc: &[Signed<Confirmation>]) -> (usize, usize) {
        let k = self.engaged.len();
        let m = self
            .engaged
            .iter()
            .filter(|w| {
                !qc.is_empty()
                    && qc
                        .iter()
                        .all(|c| self.receipts.observed(&c.hash, &w.kid).is_some())
            })
            .count();
        (m, k)
    }

    // ── Step 3h ─────────────────────────────────────────────────────────────────────────────

    /// An abandoned close at a seq with no QC (§6.8, §9.2 step 3h).
    fn abandoned_close(
        &mut self,
        seq: u64,
        view: &SeqView<R::State>,
    ) -> Result<CloseOutcome, Error> {
        let Some(prev) = self.committed.last().cloned() else {
            return Ok(CloseOutcome::NoClose);
        };
        let prev_keys = self.keys_for(prev.link.payload.seq).clone();
        let keys = self.keys_for(seq).clone();

        struct Close {
            link: Signed<Link>,
            author: PartyIndex,
            subjects: Vec<PartyIndex>,
            body: CloseBody,
        }
        let mut closes: Vec<Close> = Vec::new();
        let mut present: BTreeSet<PartyIndex> = view.present.clone();
        let mut records: BTreeMap<PartyIndex, Vec<Hash>> = view.records.clone();
        for l in self.links_by_seq.get(&seq).cloned().unwrap_or_default() {
            if l.payload.chain != self.chain
                || l.payload.kind() != Kind::Close
                || l.payload.prev_hash().ok().flatten() != Some(prev.link.hash)
                || l.payload.state != prev.link.payload.state
            {
                continue;
            }
            let Ok(body) = serde_json::from_value::<CloseBody>(l.payload.body.clone()) else {
                continue;
            };
            if body.reason != CloseReason::Abandoned {
                continue;
            }
            let Some(&author) = keys.get(&l.payload.kid) else {
                continue;
            };
            if self.genesis.parties[author].pubky != l.payload.author
                || !self.sig_ok(&l, &l.payload.kid)
            {
                continue;
            }
            if self
                .embedded_qc(&l.payload.confirms, &prev, &prev_keys)
                .is_none()
            {
                continue;
            }
            present.insert(author);
            records.entry(author).or_default().push(l.hash);
            let mut subjects: Vec<PartyIndex> = body
                .subject
                .iter()
                .filter_map(|s| self.party_by_pubky(s))
                .collect();
            subjects.sort_unstable();
            subjects.dedup();
            if subjects.is_empty()
                || subjects.len() != body.subject.len()
                || subjects.contains(&author)
            {
                self.anomaly(
                    Some(&l.payload.kid),
                    seq,
                    AnomalyKind::VoidClose,
                    vec![l.hash],
                );
                continue;
            }
            closes.push(Close {
                link: l,
                author,
                subjects,
                body,
            });
        }
        if closes.is_empty() {
            return Ok(CloseOutcome::NoClose);
        }

        // Confirmations without a round: agreement among the non-subjects.
        let mut confirmed_by: BTreeMap<Hash, BTreeMap<PartyIndex, Signed<Confirmation>>> =
            BTreeMap::new();
        let mut per_party_closes: BTreeMap<PartyIndex, BTreeSet<Hash>> = BTreeMap::new();
        for c in self.confirms_by_seq.get(&seq).cloned().unwrap_or_default() {
            if c.payload.round.is_some() {
                continue;
            }
            let Some(&party) = keys.get(&c.payload.kid) else {
                continue;
            };
            let Ok(link) = Hash::parse(&c.payload.link) else {
                continue;
            };
            if !closes.iter().any(|k| k.link.hash == link) || !self.sig_ok(&c, &c.payload.kid) {
                continue;
            }
            present.insert(party);
            records.entry(party).or_default().push(c.hash);
            confirmed_by
                .entry(link)
                .or_default()
                .entry(party)
                .or_insert(c);
            per_party_closes.entry(party).or_default().insert(link);
        }
        for (party, hashes) in &per_party_closes {
            let subject_sets: BTreeSet<Vec<PartyIndex>> = hashes
                .iter()
                .filter_map(|h| closes.iter().find(|k| k.link.hash == *h))
                .map(|k| k.subjects.clone())
                .collect();
            if subject_sets.len() > 1 {
                let kid = self.seat_kid(*party);
                self.anomaly(
                    kid.as_deref(),
                    seq,
                    AnomalyKind::Equivocation,
                    hashes.iter().copied().collect(),
                );
            }
        }

        let mut complete: Vec<usize> = (0..closes.len())
            .filter(|i| {
                let k = &closes[*i];
                let confirmers = confirmed_by.get(&k.link.hash);
                (0..self.n)
                    .filter(|p| *p != k.author && !k.subjects.contains(p))
                    .all(|p| confirmers.map(|m| m.contains_key(&p)).unwrap_or(false))
            })
            .collect();
        complete.sort_by_key(|i| closes[*i].link.hash);

        let stopwatch_view = SeqView {
            candidates: Vec::new(),
            rounds: BTreeMap::new(),
            present: present.clone(),
            records,
            vetoes: BTreeMap::new(),
            skips: Vec::new(),
        };
        let mut paused: Option<Status> = None;
        for i in complete {
            let close = &closes[i];
            let present_subjects: Vec<PartyIndex> = close
                .subjects
                .iter()
                .copied()
                .filter(|s| present.contains(s))
                .collect();

            // Adjudicate first (§6.8): the bound depends on it.
            let verdict =
                self.close_silence(close.link.hash, &close.subjects, view, &stopwatch_view);
            let adjudicated = matches!(verdict, TimeVerdict::Yes | TimeVerdict::No);
            if close.subjects.len() > max_subjects(self.n, adjudicated) {
                self.anomaly(
                    Some(&close.link.payload.kid),
                    seq,
                    AnomalyKind::VoidClose,
                    vec![close.link.hash],
                );
                continue;
            }
            match verdict {
                TimeVerdict::Yes => {
                    // Final: every subject was silent past the allowance when the close was
                    // observed. A present subject's record is late.
                    for s in &present_subjects {
                        let kid = self.seat_kid(*s);
                        let ev = stopwatch_view.records.get(s).cloned().unwrap_or_default();
                        self.anomaly(kid.as_deref(), seq, AnomalyKind::LateSubject, ev);
                    }
                    // A party who never revealed can be closed out before the rules have
                    // initialised (§6.6); there is then no state for the rules to judge.
                    let outcome = match self.state.as_ref() {
                        Some(state) => self
                            .rules
                            .close(state, &close.body)
                            .map_err(|e| Error::Rules(e.0))?,
                        None => Outcome {
                            summary: "abandoned before the rules initialised".into(),
                            winners: Vec::new(),
                        },
                    };
                    let qc: Vec<Signed<Confirmation>> = confirmed_by
                        .get(&close.link.hash)
                        .map(|m| m.values().cloned().collect())
                        .unwrap_or_default();
                    let witnessed = self.witnessed(&qc);
                    if let Some(p) = self.committed.last_mut() {
                        p.is_final = true;
                    }
                    self.committed.push(Committed {
                        link: close.link.clone(),
                        author: close.author,
                        qc,
                        is_final: true,
                        witnessed,
                    });
                    self.status = Status::Abandoned {
                        subjects: close.subjects.clone(),
                        outcome,
                    };
                    return Ok(CloseOutcome::Ended);
                }
                TimeVerdict::No => {
                    // Final: dropped.
                    self.anomaly(
                        Some(&close.link.payload.kid),
                        seq,
                        AnomalyKind::VoidClose,
                        vec![close.link.hash],
                    );
                }
                TimeVerdict::Asserted { .. } => {
                    if paused.is_none() {
                        let state = if present_subjects.is_empty() {
                            CloseState::Asserted
                        } else {
                            CloseState::Contested {
                                present: present_subjects,
                            }
                        };
                        paused = Some(Status::Paused { seq, close: state });
                    }
                }
            }
        }
        Ok(paused
            .map(CloseOutcome::Paused)
            .unwrap_or(CloseOutcome::NoClose))
    }

    /// Was every subject silent past the allowance when the close was observed (§11.3), by
    /// the witness quorum? A witness judges a subject only if it receipted the close and the
    /// record that started the subject's obligation.
    fn close_silence(
        &self,
        close: Hash,
        subjects: &[PartyIndex],
        view: &SeqView<R::State>,
        records: &SeqView<R::State>,
    ) -> TimeVerdict {
        if self.engaged.is_empty() {
            return TimeVerdict::Asserted {
                yes: 0,
                no: 0,
                silent: 0,
            };
        }
        let (think_ms, respond_ms) = self.allowances();
        let obliged: BTreeSet<PartyIndex> = self
            .state
            .as_ref()
            .map(|s| self.rules.obliged(s).into_iter().collect())
            .unwrap_or_default();
        let judgements: Vec<Option<bool>> = self
            .engaged
            .iter()
            .map(|w| {
                let t_close = self.receipts.observed(&close, &w.kid)?;
                for s in subjects {
                    // The subject's obligation: to propose, if obliged; else to vote on the
                    // earliest valid proposal by someone else. Every such proposal in the
                    // files must have been receipted by this witness, or it cannot judge.
                    let (start, allowance) = if obliged.contains(s) {
                        (self.ready_as_seen_by(&w.kid)?, think_ms)
                    } else {
                        let mut first: Option<u64> = None;
                        for c in view.candidates.iter().filter(|c| c.author != *s) {
                            let t = self.receipts.observed(&c.link.hash, &w.kid)?;
                            if t <= t_close {
                                first = Some(first.map_or(t, |f| f.min(t)));
                            }
                        }
                        match first {
                            Some(t) => (t, respond_ms),
                            None => return Some(false), // nothing was owed before the close
                        }
                    };
                    let mut start = start;
                    if let Some(t) = self.last_record_before(records, *s, &w.kid, t_close).ok()? {
                        start = start.max(t);
                    }
                    if crate::witness::stopwatch::silence(t_close, start) <= allowance {
                        return Some(false);
                    }
                }
                Some(true)
            })
            .collect();
        adjudicate(&judgements)
    }

    /// Step 3d, none: the next seq is open (no votes yet) or stalled (votes, no QC).
    fn stall(&mut self, seq: u64, view: &SeqView<R::State>) {
        if seq > 0 && view.rounds.values().all(|v| v.by_party.is_empty()) {
            self.status = Status::Ongoing;
            return;
        }
        let round = view.rounds.keys().next_back().copied().unwrap_or(0);
        let (dead, voters) = view
            .rounds
            .get(&round)
            .map(|v| (v.is_dead(self.n, self.q), v.voters()))
            .unwrap_or((false, BTreeSet::new()));
        let awaiting: Vec<PartyIndex> = (0..self.n)
            .filter(|p| self.seats[*p].is_some() && !voters.contains(p))
            .collect();
        for c in &view.candidates {
            if c.link.payload.round == round
                && view
                    .rounds
                    .get(&round)
                    .map(|v| v.votes_for(&c.link.hash) <= 1)
                    .unwrap_or(true)
            {
                self.anomaly(
                    None,
                    seq,
                    AnomalyKind::UnconfirmedProposal,
                    vec![c.link.hash],
                );
            }
        }
        self.status = Status::Stalled {
            seq,
            round,
            dead,
            awaiting,
        };
    }

    // ── Step 4 ──────────────────────────────────────────────────────────────────────────────

    /// Grant windows, by the witness quorum, after the whole fold. Returns the records that
    /// are INVALID — placed after their key's window by a quorum, placed inside it by nothing,
    /// and not part of any QC a committed successor embedded — for the caller to remove and
    /// fold again. Everything else the quorum places late is an anomaly against its signer
    /// (and, once history, against the confirmers who accepted it); `committed[]` is unchanged.
    fn grant_windows(&mut self) -> Vec<Hash> {
        if self.engaged.is_empty() {
            return Vec::new();
        }
        // History: every link and QC confirmation a committed successor has embedded.
        let mut history: HashSet<Hash> = HashSet::new();
        let mut confirmers_of: BTreeMap<Hash, Vec<String>> = BTreeMap::new();
        for c in &self.committed {
            if c.is_final {
                history.insert(c.link.hash);
                history.extend(c.qc.iter().map(|q| q.hash));
            }
            confirmers_of.insert(
                c.link.hash,
                c.qc.iter().map(|q| q.payload.kid.clone()).collect(),
            );
        }
        // Every verified record, with its signer and seq.
        let mut records: Vec<(Hash, String, u64)> = Vec::new();
        for (seq, ls) in &self.links_by_seq {
            records.extend(
                ls.iter()
                    .filter(|l| self.verified.contains(&l.hash))
                    .map(|l| (l.hash, l.payload.kid.clone(), *seq)),
            );
        }
        for (seq, cs) in &self.confirms_by_seq {
            records.extend(
                cs.iter()
                    .filter(|c| self.verified.contains(&c.hash))
                    .map(|c| (c.hash, c.payload.kid.clone(), *seq)),
            );
        }
        for (seq, rs) in &self.rejects_by_seq {
            records.extend(
                rs.iter()
                    .filter(|r| self.verified.contains(&r.hash))
                    .map(|r| (r.hash, r.payload.kid.clone(), *seq)),
            );
        }

        let engaged = self.engaged.clone();
        let mut invalid = Vec::new();
        for (hash, kid, seq) in records {
            let Some(&(_, exp)) = self.keys_seen.get(&kid) else {
                continue;
            };
            let exp_ms = exp.saturating_mul(1000);
            // The window's end as each witness saw it: the Grant's `exp`, or its own receipt
            // of a revocation of this kid if earlier.
            let bound_for = |w: &Engaged| -> u64 {
                let revoked_at = self
                    .revocations
                    .get(&kid)
                    .and_then(|r| self.receipts.observed(&r.hash, &w.kid));
                revoked_at.map_or(exp_ms, |t| t.min(exp_ms))
            };
            let judgements: Vec<Option<bool>> = engaged
                .iter()
                .map(|w| {
                    let t = self.receipts.observed(&hash, &w.kid)?;
                    Some(t > bound_for(w))
                })
                .collect();
            if adjudicate(&judgements) != TimeVerdict::Yes {
                continue;
            }
            // (i) Does anything in the fold place it inside its window? A receipt of it by any
            //     witness ever engaged, or a receipt of a counterparty's confirmation of it,
            //     at or before the bound.
            let placed = self.engagement_keys.keys().any(|wk| {
                let bound = self
                    .revocations
                    .get(&kid)
                    .and_then(|r| self.receipts.observed(&r.hash, wk))
                    .map_or(exp_ms, |t| t.min(exp_ms));
                self.receipts
                    .observed(&hash, wk)
                    .is_some_and(|t| t <= bound)
                    || self
                        .confirms_by_seq
                        .get(&seq)
                        .map(|cs| {
                            cs.iter().any(|c| {
                                Hash::parse(&c.payload.link).ok() == Some(hash)
                                    && self
                                        .receipts
                                        .observed(&c.hash, wk)
                                        .is_some_and(|t| t <= bound)
                            })
                        })
                        .unwrap_or(false)
            });
            let is_history = history.contains(&hash);
            self.anomaly(Some(&kid), seq, AnomalyKind::GrantWindow, vec![hash]);
            if is_history {
                for confirmer in confirmers_of.get(&hash).cloned().unwrap_or_default() {
                    self.anomaly(Some(&confirmer), seq, AnomalyKind::GrantWindow, vec![hash]);
                }
            } else if !placed {
                invalid.push(hash);
            }
        }
        invalid
    }

    // ── Step 5 ──────────────────────────────────────────────────────────────────────────────

    fn finish(self) -> Verdict {
        Verdict {
            chain: self.chain,
            committed: self.committed,
            status: self.status,
            seats: self.seats.into_iter().flatten().collect(),
            engaged: self.engaged,
            recoveries: self.recoveries,
            anomalies: self.anomalies,
        }
    }
}

/// What step 3h concluded.
enum CloseOutcome {
    /// No complete abandoned close at this seq.
    NoClose,
    /// An adjudicated-valid close ended the chain.
    Ended,
    /// An asserted or contested close pauses it.
    Paused(Status),
}

/// Step 1: is this link a valid genesis under `rules`? Returns the body and the initiator's
/// path (from `sources` if known, else the default folder for the Grant's `client_id`).
fn validate_genesis<R: Rules>(
    rules: &R,
    link: &Signed<Link>,
    config: &Config,
    inputs: &Inputs,
) -> Result<(Genesis, String), Error> {
    let p = &link.payload;
    if p.v != crate::PROTOCOL_VERSION || !p.chain.is_empty() || p.round != 0 {
        return Err(Error::Genesis("genesis shape".into()));
    }
    if !p.confirms.is_empty() || !p.receipts.is_empty() {
        return Err(Error::Genesis("genesis embeds a QC".into()));
    }
    let genesis: Genesis =
        serde_json::from_value(p.body.clone()).map_err(|e| Error::Genesis(format!("body: {e}")))?;
    genesis.check_safety(
        |id| (id == rules.id()).then(|| rules.reference_hash().to_string()),
        config.max_body_cap,
    )?;
    if link.bytes.len() as u64 > genesis.max_body_bytes {
        return Err(Error::Genesis(
            "genesis larger than its own max_body_bytes".into(),
        ));
    }
    let initiator = &genesis.parties[0];
    if initiator.pubky != p.author || initiator.kid.as_deref() != Some(p.kid.as_str()) {
        return Err(Error::Genesis("author is not parties[0]".into()));
    }
    let grant = p
        .grant
        .as_deref()
        .ok_or_else(|| Error::Genesis("no Grant".into()))?;
    let pubky = parse_z32(&p.author)?;
    let kid = parse_z32(&p.kid)?;
    let claims = pubky_common::auth::grant::GrantClaims::decode(grant)
        .map_err(|e| Error::Grant(format!("decode: {e}")))?;
    // The initiator's folder is the one of theirs the genesis was read from; a mirror in
    // another party's folder says nothing about where the initiator writes.
    let owned_by_initiator = format!("pubky://{}/", p.author);
    let path = inputs
        .sources
        .get(&link.hash)
        .and_then(|srcs| srcs.iter().find(|s| s.starts_with(&owned_by_initiator)))
        .map(|s| storage_path(s).to_string())
        .unwrap_or_else(|| format!("/pub/{}/{PROTOCOL_FOLDER}/", claims.client_id));
    verify_grant(grant, &pubky, &kid, &path)?;
    link.verify(&kid)?;
    Ok((genesis, path))
}
