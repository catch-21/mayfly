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
//! the two side by side. What this file does not yet do, and which test will drive it in:
//!
//! - `recover`, `reveal` and `witnesses` links are not candidates yet
//!   (`recover_veto_window`, `folders_follow_declared_paths`);
//! - every time question — abandoned-close adjudication, the recover delay, premature skips
//!   and Grant windows (step 4) — is left *asserted*: the receipts are indexed and `witnessed
//!   m/k` is reported, but the stopwatch of §11.3 is not yet consulted
//!   (`close_verdicts_never_contradict_when_final`).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use pubky_common::crypto::PublicKey;

use crate::close::{max_subjects, CloseState};
use crate::error::Error;
use crate::genesis::Genesis;
use crate::hash::{ChainId, Hash};
use crate::keys::{parse_z32, verify_grant, Seat};
use crate::record::{
    CloseBody, CloseReason, Confirmation, Engagement, KeyChangeBody, Kind, Link, Receipt, Reject,
    Signed,
};
use crate::rules::{state_hash, Outcome, PartyIndex, Rules, Status as RulesStatus};
use crate::typ;
use crate::vote::{designated, effective_quorum, RoundVotes, Vote};
use crate::witness::{Engaged, Receipts};
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
    /// Confirmations forming its QC.
    pub qc: Vec<Signed<Confirmation>>,
    /// True once a successor has embedded this QC (§6.4).
    pub is_final: bool,
    /// *Witnessed m/k*: how many of the engaged witnesses receipted every confirmation of its
    /// QC (and so its QC-completing one), over how many were engaged.
    pub witnessed: (usize, usize),
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
    /// An abandoned close that named a present party, or exceeded the subject bound.
    VoidClose,
    /// A subject record observed after an adjudicated close.
    LateSubject,
    /// A recover confirmed inside `recovery_delay_ms` (quorum-adjudicated).
    PrematureRecover,
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
    let mut fold = Fold::new(rules, inputs, config)?;
    fold.run()?;
    Ok(fold.finish())
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

/// A link that passed step 3a at its seq.
struct Candidate<S> {
    link: Signed<Link>,
    author: PartyIndex,
    kind: Kind,
    /// Rules kinds: the state after `apply`. Protocol kinds and genesis: `None`.
    next_state: Option<S>,
    /// The quorum this kind needs (§9.2 step 3b).
    quorum: usize,
    /// `rekey`: the new key and Grant expiry to establish on commit.
    rekey: Option<(String, u64)>,
    /// The link's embedded QC of `prev`, decoded and verified (empty for genesis).
    embedded_qc: Vec<Signed<Confirmation>>,
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
    receipts: Receipts,
    /// Receipt bytes waiting for their engagement (embedded ones are verified in 3e).
    pending_receipts: Vec<Signed<Receipt>>,
    verified: HashSet<Hash>,
    key_cache: BTreeMap<String, PublicKey>,
    state: Option<R::State>,
    committed: Vec<Committed>,
    status: Status,
    anomalies: Vec<Anomaly>,
}

impl<'a, R: Rules> Fold<'a, R> {
    // ── Steps 1 and 2 ───────────────────────────────────────────────────────────────────────

    fn new(rules: &'a R, inputs: &Inputs, config: &Config) -> Result<Self, Error> {
        let cap = config.max_body_cap;
        let mut seen = HashSet::new();
        let mut links: Vec<Signed<Link>> = decode_all(&inputs.links, typ::LINK, cap, &mut seen);
        let mut confirms: Vec<Signed<Confirmation>> =
            decode_all(&inputs.confirms, typ::CONFIRM, cap, &mut seen);
        let rejects: Vec<Signed<Reject>> = decode_all(&inputs.rejects, typ::REJECT, cap, &mut seen);
        let engagements: Vec<Signed<Engagement>> =
            decode_all(&inputs.engagements, typ::WITNESS, cap, &mut seen);
        let mut receipts: Vec<Signed<Receipt>> =
            decode_all(&inputs.receipts, typ::WITNESS, cap, &mut seen);

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
        });
        key_cache.insert(
            initiator_kid,
            parse_z32(&genesis.parties[0].kid.clone().expect("kid"))?,
        );

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
            receipts: Receipts::default(),
            pending_receipts: receipts,
            verified: HashSet::new(),
            key_cache,
            state: None,
            committed: Vec::new(),
            status: Status::Ongoing,
            anomalies,
        };
        fold.establish_genesis_seats();
        fold.establish_genesis_witnesses(engagements);
        fold.index_receipts();
        Ok(fold)
    }

    /// Step 2: establish every party's kid, client_id and path from their genesis confirmation.
    fn establish_genesis_seats(&mut self) {
        let genesis_hash = self.genesis_link.hash;
        let Some(confirms) = self.confirms_by_seq.get(&0).cloned() else {
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
            self.seats[party] = Some(Seat {
                pubky: iss,
                kid: c.payload.kid.clone(),
                client_id: claims.client_id.to_string(),
                paths: vec![path.clone()],
                grant_exp: claims.exp,
            });
        }
        self.keys_at.push(self.current_keys());
    }

    /// Step 2: establish the GENESIS witnesses' engagements. Witnesses seated later by a
    /// `witnesses` link are established in step 3f when that link commits.
    fn establish_genesis_witnesses(&mut self, engagements: Vec<Signed<Engagement>>) {
        for w in &self.genesis.witnesses {
            let Ok(pubky_pk) = parse_z32(&w.pubky) else {
                continue;
            };
            let mut mine: Vec<(Engaged, Hash)> = Vec::new();
            for e in &engagements {
                if e.payload.chain != self.chain || e.payload.kind != "engage" {
                    continue;
                }
                let Ok(claims) = pubky_common::auth::grant::GrantClaims::decode(&e.payload.grant)
                else {
                    continue;
                };
                if claims.iss.z32() != w.pubky {
                    continue;
                }
                let Ok(kid_pk) = parse_z32(&e.payload.kid) else {
                    continue;
                };
                if verify_grant(&e.payload.grant, &pubky_pk, &kid_pk, &e.payload.path).is_err()
                    || e.verify(&kid_pk).is_err()
                {
                    continue;
                }
                self.engagement_keys.insert(e.payload.kid.clone(), kid_pk);
                mine.push((
                    Engaged {
                        pubky: w.pubky.clone(),
                        kid: e.payload.kid.clone(),
                        until: e.payload.until,
                        poll_ms: e.payload.policy.poll_ms,
                        path: e.payload.path.clone(),
                    },
                    e.hash,
                ));
            }
            if mine.is_empty() {
                continue;
            }
            let untils: BTreeSet<u64> = mine.iter().map(|(m, _)| m.until).collect();
            if untils.len() > 1 {
                self.anomalies.push(Anomaly {
                    against: Some(w.pubky.clone()),
                    seq: None,
                    kind: AnomalyKind::WitnessEquivocation,
                    evidence: mine.iter().map(|(_, h)| *h).collect(),
                });
            }
            // The later `until` governs (§11.2).
            let (governing, _) = mine
                .into_iter()
                .max_by_key(|(m, h)| (m.until, *h))
                .expect("non-empty");
            self.engaged.push(governing);
        }
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

    // ── Helpers ─────────────────────────────────────────────────────────────────────────────

    fn current_keys(&self) -> BTreeMap<String, PartyIndex> {
        self.seats
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|s| (s.kid.clone(), i)))
            .collect()
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
        prev: &Signed<Link>,
        prev_author: PartyIndex,
        prev_quorum: usize,
        keys: &BTreeMap<String, PartyIndex>,
    ) -> Option<Vec<Signed<Confirmation>>> {
        let mut out = Vec::new();
        let mut voters: BTreeSet<PartyIndex> = BTreeSet::new();
        voters.insert(prev_author);
        for c in confirms {
            let s = decode_confirmation(c.as_bytes().to_vec()).ok()?;
            if s.payload.chain != self.chain
                || s.payload.seq != prev.payload.seq
                || s.payload.round != Some(prev.payload.round)
                || Hash::parse(&s.payload.link).ok()? != prev.hash
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
        (voters.len() >= prev_quorum).then_some(out)
    }

    /// The quorum a committed link needed.
    fn quorum_of(&self, link: &Link) -> usize {
        effective_quorum(link.kind(), self.n, self.q)
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
                    rekey: None,
                    embedded_qc: Vec::new(),
                }]
            } else {
                self.candidates(seq)
            };
            let mut rounds: BTreeMap<u32, RoundVotes> = BTreeMap::new();
            let mut present: BTreeSet<PartyIndex> = BTreeSet::new();
            self.votes(seq, &candidates, &mut rounds, &mut present);
            self.unjustified_rounds(seq, &candidates, &rounds);

            match self.decide(seq, &candidates, &rounds)? {
                Decision::History { index, qc } | Decision::Head { index, qc } => {
                    let cand = &candidates[index];
                    self.report_late_votes(seq, cand.link.hash, &candidates, &rounds);
                    let stop = self.commit(seq, cand, qc)?;
                    if stop {
                        return Ok(());
                    }
                    seq += 1;
                }
                Decision::None => {
                    if seq > 0 {
                        if let Some(paused) = self.abandoned_close(seq, &present) {
                            self.status = paused;
                            return Ok(());
                        }
                    }
                    self.stall(seq, &candidates, &rounds);
                    return Ok(());
                }
            }
        }
    }

    /// Step 3a: candidates at `seq`.
    fn candidates(&mut self, seq: u64) -> Vec<Candidate<R::State>> {
        let prev = self.committed.last().expect("seq > 0 has a head").clone();
        let prev_author = *self
            .keys_for(prev.link.payload.seq)
            .get(&prev.link.payload.kid)
            .expect("committed author is seated");
        let prev_quorum = self.quorum_of(&prev.link.payload);
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
            let Some(&author) = keys.get(&l.payload.kid) else {
                continue;
            };
            if self.genesis.parties[author].pubky != l.payload.author {
                continue;
            }
            if !self.sig_ok(&l, &l.payload.kid) {
                continue;
            }
            let Some(embedded) = self.embedded_qc(
                &l.payload.confirms,
                &prev.link,
                prev_author,
                prev_quorum,
                &prev_keys,
            ) else {
                continue;
            };
            let kind = l.payload.kind();
            // Round >= 1: the designated party, except reveal (fixed author) and abandoned
            // closes (outside rounds; step h).
            let is_abandoned = kind == Kind::Close
                && serde_json::from_value::<CloseBody>(l.payload.body.clone())
                    .map(|b| b.reason == CloseReason::Abandoned)
                    .unwrap_or(false);
            if is_abandoned {
                continue;
            }
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
                rekey: None,
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
                    cand.rekey = Some((body.new_kid, claims.exp));
                }
                // Not yet candidates; see the module documentation.
                Kind::Recover | Kind::Reveal | Kind::Witnesses => continue,
            }
            out.push(cand);
        }
        out
    }

    /// Steps 3b–3c: votes per `(party, round)`, with presence for step h.
    fn votes(
        &mut self,
        seq: u64,
        candidates: &[Candidate<R::State>],
        rounds: &mut BTreeMap<u32, RoundVotes>,
        present: &mut BTreeSet<PartyIndex>,
    ) {
        let keys = self.keys_for(seq).clone();
        let mut evidence: BTreeMap<(u32, PartyIndex), Vec<Hash>> = BTreeMap::new();
        for c in candidates {
            rounds
                .entry(c.link.payload.round)
                .or_default()
                .propose(c.author, c.link.hash);
            evidence
                .entry((c.link.payload.round, c.author))
                .or_default()
                .push(c.link.hash);
            present.insert(c.author);
        }
        let by_hash: BTreeMap<Hash, &Candidate<R::State>> =
            candidates.iter().map(|c| (c.link.hash, c)).collect();

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
            present.insert(party);
            let Some(round) = c.payload.round else {
                continue; // an abandoned-close confirmation: step h
            };
            if seq == 0 && link != self.genesis_link.hash {
                continue;
            }
            rounds
                .entry(round)
                .or_default()
                .cast(party, Vote::For(link));
            evidence.entry((round, party)).or_default().push(c.hash);
            if let Some(cand) = by_hash.get(&link) {
                if seq > 0 && cand.link.payload.state != c.payload.state {
                    self.anomaly(
                        Some(&c.payload.kid),
                        seq,
                        AnomalyKind::RulesDivergence,
                        vec![cand.link.hash, c.hash],
                    );
                }
            }
        }

        let mut skipped: BTreeSet<PartyIndex> = BTreeSet::new();
        for r in self.rejects_by_seq.get(&seq).cloned().unwrap_or_default() {
            let Some(&party) = keys.get(&r.payload.kid) else {
                continue;
            };
            if !self.sig_ok(&r, &r.payload.kid) {
                continue;
            }
            present.insert(party);
            let round = r.payload.round;
            if r.payload.is_empty() {
                let is_pass = round >= 1 && designated(&self.chain, seq, round, self.n) == party;
                if !is_pass && (round == 0 || !skipped.insert(party)) {
                    self.anomaly(
                        Some(&r.payload.kid),
                        seq,
                        AnomalyKind::InvalidSkip,
                        vec![r.hash],
                    );
                    continue;
                }
            } else if let Ok(target) = Hash::parse(&r.payload.link) {
                let proposals_in_round = candidates
                    .iter()
                    .filter(|c| c.link.payload.round == round)
                    .count();
                if by_hash.contains_key(&target) && proposals_in_round == 1 {
                    self.anomaly(
                        Some(&r.payload.kid),
                        seq,
                        AnomalyKind::Obstruction,
                        vec![target, r.hash],
                    );
                }
            }
            rounds.entry(round).or_default().cast(party, Vote::Nothing);
            evidence.entry((round, party)).or_default().push(r.hash);
        }

        for (round, votes) in rounds.iter() {
            for party in votes.equivocators() {
                let kid = self.seats[party].as_ref().map(|s| s.kid.clone());
                let ev = evidence.get(&(*round, party)).cloned().unwrap_or_default();
                self.anomaly(kid.as_deref(), seq, AnomalyKind::Equivocation, ev);
            }
        }
    }

    /// Step 3g: a round `r > 0` whose predecessor is not dead in the files.
    fn unjustified_rounds(
        &mut self,
        seq: u64,
        candidates: &[Candidate<R::State>],
        rounds: &BTreeMap<u32, RoundVotes>,
    ) {
        for c in candidates {
            let r = c.link.payload.round;
            if r == 0 {
                continue;
            }
            let dead = rounds
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

    /// Step 3d.
    fn decide(
        &mut self,
        seq: u64,
        candidates: &[Candidate<R::State>],
        rounds: &BTreeMap<u32, RoundVotes>,
    ) -> Result<Decision, Error> {
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
            let Some(&s_author) = keys_after.get(&s.payload.kid) else {
                continue;
            };
            if self.genesis.parties[s_author].pubky != s.payload.author
                || !self.sig_ok(&s, &s.payload.kid)
            {
                continue;
            }
            let Some(qc) = self.embedded_qc(
                &s.payload.confirms,
                &cand.link,
                cand.author,
                cand.quorum,
                &keys_now,
            ) else {
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
        for (round, votes) in rounds.iter().rev() {
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

    /// The key map after `cand` commits (a `rekey` changes its author's key).
    fn keys_after(&self, cand: &Candidate<R::State>) -> BTreeMap<String, PartyIndex> {
        let mut keys = self.current_keys();
        if let Some((new_kid, _)) = &cand.rekey {
            keys.retain(|_, p| *p != cand.author);
            keys.insert(new_kid.clone(), cand.author);
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

    /// History: any other QC at this seq is a late vote, never an input.
    fn report_late_votes(
        &mut self,
        seq: u64,
        committed: Hash,
        candidates: &[Candidate<R::State>],
        rounds: &BTreeMap<u32, RoundVotes>,
    ) {
        for c in candidates {
            if c.link.hash == committed {
                continue;
            }
            let Some(votes) = rounds.get(&c.link.payload.round) else {
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
        if let Some(prev) = self.committed.last_mut() {
            prev.is_final = true;
        }
        self.committed.push(Committed {
            link: cand.link.clone(),
            qc: qc.clone(),
            is_final: false,
            witnessed: (m, k),
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
                Kind::Rekey => {
                    let (new_kid, exp) = cand.rekey.clone().expect("checked in 3a");
                    let seat = self.seats[cand.author].as_mut().expect("seated");
                    seat.kid = new_kid;
                    seat.grant_exp = exp;
                }
                Kind::Recover | Kind::Reveal | Kind::Witnesses => unreachable!("not candidates"),
            }
        }
        self.keys_at.push(self.current_keys());
        self.index_receipts();
        Ok(ended)
    }

    /// Step 3h: an abandoned close at a seq with no QC. Adjudication is not yet consulted, so
    /// every complete close is *asserted* or *contested* (§6.8) and pauses the chain.
    fn abandoned_close(
        &mut self,
        seq: u64,
        present_from_votes: &BTreeSet<PartyIndex>,
    ) -> Option<Status> {
        let prev = self.committed.last()?.clone();
        let prev_author = *self
            .keys_for(prev.link.payload.seq)
            .get(&prev.link.payload.kid)?;
        let prev_quorum = self.quorum_of(&prev.link.payload);
        let prev_keys = self.keys_for(prev.link.payload.seq).clone();
        let keys = self.keys_for(seq).clone();
        let party_by_pubky: BTreeMap<String, PartyIndex> = self
            .genesis
            .parties
            .iter()
            .enumerate()
            .map(|(i, p)| (p.pubky.clone(), i))
            .collect();

        struct Close {
            link: Signed<Link>,
            author: PartyIndex,
            subjects: Vec<PartyIndex>,
        }
        let mut closes: Vec<Close> = Vec::new();
        let mut present: BTreeSet<PartyIndex> = present_from_votes.clone();
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
                .embedded_qc(
                    &l.payload.confirms,
                    &prev.link,
                    prev_author,
                    prev_quorum,
                    &prev_keys,
                )
                .is_none()
            {
                continue;
            }
            present.insert(author);
            let mut subjects: Vec<PartyIndex> = body
                .subject
                .iter()
                .filter_map(|s| party_by_pubky.get(s).copied())
                .collect();
            subjects.sort_unstable();
            subjects.dedup();
            let void = subjects.is_empty()
                || subjects.len() != body.subject.len()
                || subjects.contains(&author)
                || subjects.len() > max_subjects(self.n, false);
            if void {
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
            });
        }
        if closes.is_empty() {
            return None;
        }

        // Confirmations without a round: agreement among the non-subjects.
        let mut confirmed_by: BTreeMap<Hash, BTreeSet<PartyIndex>> = BTreeMap::new();
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
            confirmed_by.entry(link).or_default().insert(party);
            per_party_closes.entry(party).or_default().insert(link);
        }
        for (party, hashes) in &per_party_closes {
            let subject_sets: BTreeSet<Vec<PartyIndex>> = hashes
                .iter()
                .filter_map(|h| closes.iter().find(|k| k.link.hash == *h))
                .map(|k| k.subjects.clone())
                .collect();
            if subject_sets.len() > 1 {
                let kid = self.seats[*party].as_ref().map(|s| s.kid.clone());
                self.anomaly(
                    kid.as_deref(),
                    seq,
                    AnomalyKind::Equivocation,
                    hashes.iter().copied().collect(),
                );
            }
        }

        let mut complete: Vec<&Close> = closes
            .iter()
            .filter(|k| {
                let confirmers = confirmed_by.get(&k.link.hash).cloned().unwrap_or_default();
                (0..self.n)
                    .filter(|p| *p != k.author && !k.subjects.contains(p))
                    .all(|p| confirmers.contains(&p))
            })
            .collect();
        complete.sort_by_key(|k| k.link.hash);
        let close = complete.first()?;
        let present_subjects: Vec<PartyIndex> = close
            .subjects
            .iter()
            .copied()
            .filter(|s| present.contains(s))
            .collect();
        let state = if present_subjects.is_empty() {
            CloseState::Asserted
        } else {
            CloseState::Contested {
                present: present_subjects,
            }
        };
        Some(Status::Paused { seq, close: state })
    }

    /// Step 3d, none: the next seq is open (no votes yet) or stalled (votes, no QC).
    fn stall(
        &mut self,
        seq: u64,
        candidates: &[Candidate<R::State>],
        rounds: &BTreeMap<u32, RoundVotes>,
    ) {
        if seq > 0 && rounds.values().all(|v| v.by_party.is_empty()) {
            self.status = Status::Ongoing;
            return;
        }
        let round = rounds.keys().next_back().copied().unwrap_or(0);
        let (dead, voters) = rounds
            .get(&round)
            .map(|v| (v.is_dead(self.n, self.q), v.voters()))
            .unwrap_or((false, BTreeSet::new()));
        let awaiting: Vec<PartyIndex> = (0..self.n)
            .filter(|p| self.seats[*p].is_some() && !voters.contains(p))
            .collect();
        for c in candidates {
            if c.link.payload.round == round
                && rounds
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

    // ── Step 5 ──────────────────────────────────────────────────────────────────────────────

    fn finish(self) -> Verdict {
        Verdict {
            chain: self.chain,
            committed: self.committed,
            status: self.status,
            seats: self.seats.into_iter().flatten().collect(),
            engaged: self.engaged,
            anomalies: self.anomalies,
        }
    }
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
