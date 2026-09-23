//! Plain-data views of a chain, for rendering (§14).
//!
//! The fold's [`Verdict`] carries signed bytes and decoded records; a page wants names, hashes
//! in one spelling, and JSON it can print. Everything here is `Serialize`, rules-agnostic, and
//! built from a verdict the same way whoever verified it did — so a viewer, a party's app and
//! the demo's explorer all render the same facts. [`decode_record`] reads one file for the
//! evidence panel: the header, the payload with embedded records unpacked, and the checks on
//! the bytes.

use base64::Engine;
use serde::Serialize;
use serde_json::Value;

use pubky_mayfly::fold::{self, Status, Verdict};
use pubky_mayfly::hash::Hash;
use pubky_mayfly::rules::{PartyIndex, Rules};

use crate::chain::Action;
use crate::store::Listed;

/// A committed link.
#[derive(Debug, Clone, Serialize)]
pub struct LinkView {
    /// Sequence number.
    pub seq: u64,
    /// The round it was committed in.
    pub round: u32,
    /// `kind`.
    pub kind: String,
    /// The link body as written.
    pub body: Value,
    /// Author's index in genesis order.
    pub author: PartyIndex,
    /// Author's pubky.
    pub author_pubky: String,
    /// Hash as unpadded base64url — what [`crate::ChainClient::confirm`] and friends accept.
    pub hash: String,
    /// Hash as the file name's 16-character prefix.
    pub h16: String,
    /// Author's own timestamp, Unix milliseconds; display only.
    pub ts: u64,
    /// Confirmers forming the QC, by party index.
    pub confirmers: Vec<PartyIndex>,
    /// *Witnessed m/k*.
    pub witnessed: (usize, usize),
    /// True once a successor embedded this QC.
    pub is_final: bool,
}

/// A valid candidate at the open seq.
#[derive(Debug, Clone, Serialize)]
pub struct CandidateView {
    /// Hash, base64url.
    pub hash: String,
    /// Hash, `h16`.
    pub h16: String,
    /// Author's index.
    pub author: PartyIndex,
    /// `kind`.
    pub kind: String,
    /// Round proposed in.
    pub round: u32,
    /// Distinct parties who voted for it in that round.
    pub votes: usize,
}

/// The open seq.
#[derive(Debug, Clone, Serialize)]
pub struct OpenSeqView {
    /// The seq.
    pub seq: u64,
    /// Highest round with a vote.
    pub round: u32,
    /// Whether that round is dead.
    pub dead: bool,
    /// Every valid candidate.
    pub candidates: Vec<CandidateView>,
    /// Parties who voted, per round: `(round, parties)`.
    pub voters: Vec<(u32, Vec<PartyIndex>)>,
}

/// An anomaly, attributed.
#[derive(Debug, Clone, Serialize)]
pub struct AnomalyView {
    /// `AnomalyKind` as text, e.g. `TamperedMirror`.
    pub kind: String,
    /// The key it is attributed to, if any.
    pub against: Option<String>,
    /// Which party or witness that key belongs to at the head, if known.
    pub against_pubky: Option<String>,
    /// The seq concerned, if any.
    pub seq: Option<u64>,
    /// `h16` of the records that prove it.
    pub evidence: Vec<String>,
}

/// A seat established at the head.
#[derive(Debug, Clone, Serialize)]
pub struct SeatView {
    /// The party's pubky.
    pub pubky: String,
    /// Its current chain key.
    pub kid: String,
    /// The app holding the seat.
    pub client_id: String,
    /// Folders its records are read from.
    pub paths: Vec<String>,
    /// Grant expiry, Unix seconds.
    pub grant_exp: u64,
}

/// A witness engaged at the head.
#[derive(Debug, Clone, Serialize)]
pub struct EngagedView {
    /// The watchdog's pubky.
    pub pubky: String,
    /// Its signing key.
    pub kid: String,
    /// End of engagement, Unix seconds.
    pub until: u64,
    /// Its polling interval.
    pub poll_ms: u64,
}

/// Where the chain stands, flattened for display.
#[derive(Debug, Clone, Serialize)]
pub struct StatusView {
    /// `ongoing`, `stalled`, `paused`, `closed` or `abandoned`.
    pub kind: &'static str,
    /// One line, e.g. `stalled at seq 4, round 1 (dead) — awaiting 0, 2`.
    pub summary: String,
    /// Parties awaited (`stalled`) or closed out (`abandoned`).
    pub parties: Vec<PartyIndex>,
    /// The rules' outcome summary, when ended.
    pub outcome: Option<String>,
}

/// A file whose name does not match its bytes (§7).
#[derive(Debug, Clone, Serialize)]
pub struct SuspectView {
    /// Folder owner.
    pub owner: String,
    /// Absolute path.
    pub path: String,
}

/// The chain as verified from files.
#[derive(Debug, Clone, Serialize)]
pub struct ChainView {
    /// Chain id.
    pub chain: String,
    /// Rules id pinned in genesis.
    pub rules: Option<String>,
    /// Every party's pubky, in genesis order.
    pub parties: Vec<String>,
    /// Where the chain stands.
    pub status: StatusView,
    /// Closed or adjudicated-abandoned.
    pub is_final: bool,
    /// Committed links, seq 0 upward.
    pub committed: Vec<LinkView>,
    /// The open seq, when ongoing or stalled.
    pub open: Option<OpenSeqView>,
    /// Seats at the head.
    pub seats: Vec<SeatView>,
    /// Witnesses at the head.
    pub engaged: Vec<EngagedView>,
    /// Anomalies, attributed.
    pub anomalies: Vec<AnomalyView>,
    /// Files whose bytes do not match their name.
    pub suspects: Vec<SuspectView>,
    /// The rules state at the head, if the rules have initialised.
    pub state: Option<Value>,
}

/// Build a [`ChainView`] from a verdict, replaying the rules for the state.
pub fn chain_view<R: Rules>(rules: &R, v: &Verdict, suspects: &[Listed]) -> ChainView {
    let genesis = v.genesis();
    let parties: Vec<String> = genesis
        .as_ref()
        .map(|g| g.parties.iter().map(|p| p.pubky.clone()).collect())
        .unwrap_or_default();
    let party_of_kid = |kid: &str| -> Option<PartyIndex> {
        v.seats
            .iter()
            .find(|s| s.kid == kid)
            .and_then(|s| parties.iter().position(|p| *p == s.pubky))
    };
    let pubky_of_kid = |kid: &str| -> Option<String> {
        v.seats
            .iter()
            .find(|s| s.kid == kid)
            .map(|s| s.pubky.clone())
            .or_else(|| {
                v.engaged
                    .iter()
                    .find(|w| w.kid == kid)
                    .map(|w| w.pubky.clone())
            })
    };
    let committed = v
        .committed
        .iter()
        .map(|c| LinkView {
            seq: c.link.payload.seq,
            round: c.link.payload.round,
            kind: c.link.payload.kind.clone(),
            body: c.link.payload.body.clone(),
            author: c.author,
            author_pubky: c.link.payload.author.clone(),
            hash: c.link.hash.to_base64url(),
            h16: c.link.hash.h16(),
            ts: c.link.payload.ts,
            confirmers: c
                .qc
                .iter()
                .filter_map(|q| party_of_kid(&q.payload.kid))
                .collect(),
            witnessed: c.witnessed,
            is_final: c.is_final,
        })
        .collect();
    let open = v.open.as_ref().map(|o| OpenSeqView {
        seq: o.seq,
        round: o.round,
        dead: o.dead,
        candidates: o
            .candidates
            .iter()
            .map(|c| CandidateView {
                hash: c.hash.to_base64url(),
                h16: c.hash.h16(),
                author: c.author,
                kind: c.kind.clone(),
                round: c.round,
                votes: c.votes,
            })
            .collect(),
        voters: o.voters.iter().map(|(r, ps)| (*r, ps.clone())).collect(),
    });
    let state = fold::replay_state(rules, v)
        .ok()
        .flatten()
        .and_then(|s| serde_json::to_value(s).ok());
    ChainView {
        chain: v.chain.to_string(),
        rules: genesis.as_ref().map(|g| g.rules.clone()),
        status: status_view(&v.status),
        is_final: v.is_final(),
        committed,
        open,
        seats: v
            .seats
            .iter()
            .map(|s| SeatView {
                pubky: s.pubky.clone(),
                kid: s.kid.clone(),
                client_id: s.client_id.clone(),
                paths: s.paths.clone(),
                grant_exp: s.grant_exp,
            })
            .collect(),
        engaged: v
            .engaged
            .iter()
            .map(|w| EngagedView {
                pubky: w.pubky.clone(),
                kid: w.kid.clone(),
                until: w.until,
                poll_ms: w.poll_ms,
            })
            .collect(),
        anomalies: v
            .anomalies
            .iter()
            .map(|a| AnomalyView {
                kind: format!("{:?}", a.kind),
                against_pubky: a.against.as_deref().and_then(pubky_of_kid),
                against: a.against.clone(),
                seq: a.seq,
                evidence: a.evidence.iter().map(Hash::h16).collect(),
            })
            .collect(),
        suspects: suspects
            .iter()
            .map(|s| SuspectView {
                owner: s.owner.clone(),
                path: s.path.clone(),
            })
            .collect(),
        state,
        parties,
    }
}

/// Flatten a [`Status`].
pub fn status_view(s: &Status) -> StatusView {
    let list = |ps: &[PartyIndex]| {
        ps.iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match s {
        Status::Ongoing => StatusView {
            kind: "ongoing",
            summary: "ongoing".into(),
            parties: Vec::new(),
            outcome: None,
        },
        Status::Stalled {
            seq,
            round,
            dead,
            awaiting,
        } => StatusView {
            kind: "stalled",
            summary: format!(
                "stalled at seq {seq}, round {round}{} — awaiting {}",
                if *dead { " (dead)" } else { "" },
                list(awaiting)
            ),
            parties: awaiting.clone(),
            outcome: None,
        },
        Status::Paused { seq, close } => StatusView {
            kind: "paused",
            summary: format!("paused at seq {seq}: abandoned close {close:?}").to_lowercase(),
            parties: Vec::new(),
            outcome: None,
        },
        Status::Closed(o) => StatusView {
            kind: "closed",
            summary: format!("closed — {}", o.summary),
            parties: Vec::new(),
            outcome: Some(o.summary.clone()),
        },
        Status::Abandoned { subjects, outcome } => StatusView {
            kind: "abandoned",
            summary: format!("abandoned by {} — {}", list(subjects), outcome.summary),
            parties: subjects.clone(),
            outcome: Some(outcome.summary.clone()),
        },
    }
}

/// What [`crate::ChainClient::act`] did, as data.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActionView {
    /// Confirmed a valid rules proposal.
    Confirmed {
        /// The link, base64url.
        hash: String,
    },
    /// Re-proposed after a dead round.
    Reproposed {
        /// The earlier candidate.
        earlier: String,
        /// My new link.
        link: String,
    },
    /// Passed as designated proposer.
    Passed {
        /// The pass record.
        hash: String,
    },
    /// Skipped a silent designated proposer.
    Skipped {
        /// The skip record.
        hash: String,
    },
    /// Rejected a link the fold does not admit.
    Rejected {
        /// The refused link.
        link: String,
        /// Why.
        reason: String,
    },
    /// A candidate the user must decide on.
    Decision {
        /// The candidate.
        candidate: CandidateView,
        /// The round I would vote in.
        round: u32,
        /// Whether the decision is to re-propose rather than confirm.
        repropose: bool,
    },
    /// My round to propose in.
    MyTurn {
        /// The round.
        round: u32,
    },
    /// Holding my vote for witnesses.
    AwaitingWitnesses {
        /// Receipts held.
        have: usize,
        /// Engaged witnesses.
        of: usize,
        /// Policy.
        want: usize,
    },
}

impl From<&Action> for ActionView {
    fn from(a: &Action) -> Self {
        match a {
            Action::Confirmed(h) => Self::Confirmed {
                hash: h.to_base64url(),
            },
            Action::Reproposed { earlier, link } => Self::Reproposed {
                earlier: earlier.to_base64url(),
                link: link.to_base64url(),
            },
            Action::Passed(h) => Self::Passed {
                hash: h.to_base64url(),
            },
            Action::Skipped(h) => Self::Skipped {
                hash: h.to_base64url(),
            },
            Action::Rejected { link, reason } => Self::Rejected {
                link: link.to_base64url(),
                reason: reason.clone(),
            },
            Action::Decision {
                candidate,
                round,
                repropose,
            } => Self::Decision {
                candidate: CandidateView {
                    hash: candidate.hash.to_base64url(),
                    h16: candidate.hash.h16(),
                    author: candidate.author,
                    kind: candidate.kind.clone(),
                    round: candidate.round,
                    votes: candidate.votes,
                },
                round: *round,
                repropose: *repropose,
            },
            Action::MyTurn { round } => Self::MyTurn { round: *round },
            Action::AwaitingWitnesses { have, of, want } => Self::AwaitingWitnesses {
                have: *have,
                of: *of,
                want: *want,
            },
        }
    }
}

/// One record file, decoded for inspection: what the bytes say and whether they check out.
#[derive(Debug, Clone, Serialize)]
pub struct RecordView {
    /// The raw JWS compact string as stored — the identity.
    pub raw: String,
    /// `BLAKE3(bytes)`, base64url.
    pub hash: String,
    /// The same, as `h16`.
    pub h16: String,
    /// JWS header `typ`, when the file is a well-formed record.
    pub typ: Option<String>,
    /// The payload, with every embedded JWS (`confirms`, `receipts`, `grant`, `engage`)
    /// unpacked into `{typ, payload, sig}` for reading. Display only; the verifier reads bytes.
    pub payload: Value,
    /// The key the record claims (`kid`, or `by` for a revocation).
    pub claimed_key: Option<String>,
    /// Whether the Ed25519 signature verifies under the claimed key. `None` if there is none
    /// or it does not parse. Whether that key is *seated* is the fold's question.
    pub signature_ok: Option<bool>,
    /// Whether `BLAKE3(bytes)` matches the hash the store reported (the homeserver's `ETag`).
    pub hash_matches_etag: Option<bool>,
    /// Whether the file name's `<h16>` matches the bytes (§7); `None` when the name carries
    /// no hash.
    pub hash_matches_name: Option<bool>,
    /// Why the file could not be read as a record, if it could not.
    pub error: Option<String>,
}

/// Decode one record file: split the JWS, unpack the payload and every JWS embedded in it,
/// check the signature under the key the record names, and check the bytes against `etag`
/// and against the hash segment of `name` (the file's path or base name).
pub fn decode_record(bytes: &[u8], name: &str, etag: Option<&Hash>) -> RecordView {
    let hash = Hash::of(bytes);
    let raw = String::from_utf8_lossy(bytes).into_owned();
    let hash_matches_name = name
        .rsplit('/')
        .next()
        .and_then(|n| n.strip_suffix(".jws"))
        .and_then(|n| n.split('-').find(|seg| seg.len() == 16))
        .map(|seg| seg == hash.h16());
    let mut view = RecordView {
        raw: raw.clone(),
        hash: hash.to_base64url(),
        h16: hash.h16(),
        typ: None,
        payload: Value::Null,
        claimed_key: None,
        signature_ok: None,
        hash_matches_etag: etag.map(|e| *e == hash),
        hash_matches_name,
        error: None,
    };
    let (header, payload, signing_input, signature) = match split_jws(&raw) {
        Ok(parts) => parts,
        Err(e) => {
            view.error = Some(e);
            return view;
        }
    };
    view.typ = header
        .get("typ")
        .and_then(|t| t.as_str())
        .map(str::to_string);
    view.claimed_key = payload
        .get("kid")
        .or_else(|| payload.get("by"))
        .and_then(|k| k.as_str())
        .map(str::to_string);
    view.signature_ok = view.claimed_key.as_deref().and_then(|k| {
        let key = pubky_common::crypto::PublicKey::try_from(k).ok()?;
        let sig = pubky_common::crypto::Signature::from_slice(&signature).ok()?;
        Some(key.verify(signing_input.as_bytes(), &sig).is_ok())
    });
    view.payload = unpack_embedded(payload);
    view
}

/// `(header, payload, signing_input, signature)` of a compact JWS, or why not.
fn split_jws(compact: &str) -> Result<(Value, Value, String, Vec<u8>), String> {
    let url = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let parts: Vec<&str> = compact.trim().split('.').collect();
    let [h, p, s] = parts.as_slice() else {
        return Err("not a JWS: expected three dot-separated parts".into());
    };
    let header: Value = url
        .decode(h)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or("header is not base64url JSON")?;
    let payload: Value = url
        .decode(p)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or("payload is not base64url JSON")?;
    let signature = url.decode(s).map_err(|_| "signature is not base64url")?;
    Ok((header, payload, format!("{h}.{p}"), signature))
}

/// Replace every string that is itself a compact JWS with `{typ, payload, sig}` so embedded
/// confirmations, receipts, Grants and engagements read as what they are. Recursive, so a
/// confirmation embedded in a link shows its own Grant unpacked too.
fn unpack_embedded(v: Value) -> Value {
    match v {
        Value::String(s) => match split_jws(&s) {
            Ok((header, payload, _, sig)) if header.get("alg").is_some() => {
                let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&sig);
                serde_json::json!({
                    "typ": header.get("typ").cloned().unwrap_or(Value::Null),
                    "payload": unpack_embedded(payload),
                    "sig": format!("{}…", &sig_b64[..sig_b64.len().min(12)]),
                })
            }
            _ => Value::String(s),
        },
        Value::Array(items) => Value::Array(items.into_iter().map(unpack_embedded).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, unpack_embedded(v)))
                .collect(),
        ),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pubky_common::crypto::Keypair;
    use pubky_mayfly::typ;

    #[test]
    fn a_signed_record_decodes_verifies_and_unpacks_embedded_jws() {
        let confirmer = Keypair::random();
        let confirm = pubky_mayfly::record::sign(
            &confirmer,
            typ::CONFIRM,
            &serde_json::json!({ "v": 1, "seq": 1, "kid": confirmer.public_key().z32() }),
        )
        .unwrap();
        let author = Keypair::random();
        let link = pubky_mayfly::record::sign(
            &author,
            typ::LINK,
            &serde_json::json!({
                "v": 1, "seq": 2, "kid": author.public_key().z32(),
                "confirms": [String::from_utf8(confirm).unwrap()],
            }),
        )
        .unwrap();
        let hash = Hash::of(&link);
        let name = format!("chains/x/links/00000002-{}.jws", hash.h16());
        let view = decode_record(&link, &name, Some(&hash));
        assert_eq!(view.typ.as_deref(), Some(typ::LINK));
        assert_eq!(view.signature_ok, Some(true));
        assert_eq!(view.hash_matches_etag, Some(true));
        assert_eq!(view.hash_matches_name, Some(true));
        assert_eq!(view.payload["confirms"][0]["typ"], typ::CONFIRM);
        assert_eq!(view.payload["confirms"][0]["payload"]["seq"], 1);

        // Swap the first character of the signature for another valid base64url character
        // (the last one carries only two significant bits, which a lenient decoder ignores).
        let mut tampered = link.clone();
        let at = tampered.iter().rposition(|b| *b == b'.').unwrap() + 1;
        tampered[at] = if tampered[at] == b'A' { b'B' } else { b'A' };
        let view = decode_record(&tampered, &name, Some(&hash));
        assert_eq!(view.signature_ok, Some(false));
        assert_eq!(view.hash_matches_etag, Some(false));
        assert_eq!(view.hash_matches_name, Some(false));
    }

    #[test]
    fn mirrored_confirmation_names_carry_the_hash_in_the_second_segment() {
        let bytes = b"not a record";
        let hash = Hash::of(bytes);
        let name = format!("confirms/00000007-{}-somekid.jws", hash.h16());
        let view = decode_record(bytes, &name, None);
        assert_eq!(view.hash_matches_name, Some(true));
        assert!(view.error.is_some());
        assert_eq!(view.hash_matches_etag, None);
    }
}
