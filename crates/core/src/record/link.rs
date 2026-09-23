//! Link payload (§6.1) and the protocol-level kinds (§6.6–6.8, §11.2).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::hash::{ChainId, Hash};

/// A proposed link. Proposing it is the author's vote for it in `round` (§6.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    /// Protocol version.
    pub v: u32,
    /// The chain this link belongs to. Empty on genesis, whose bytes define the id (§6.5).
    #[serde(default, skip_serializing_if = "ChainId::is_empty")]
    pub chain: ChainId,
    /// Sequence number; genesis is 0.
    pub seq: u64,
    /// Voting round at this seq in which the link is proposed; almost always 0.
    pub round: u32,
    /// Hash of the committed link at `seq - 1`; empty for genesis.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prev: String,
    /// The quorum certificate of `prev`: the confirmation records that committed it, as JWS
    /// strings, sorted by confirmer `kid` (byte order of the z32 string). Empty for genesis.
    #[serde(default)]
    pub confirms: Vec<String>,
    /// Watchman receipts of the QC-completing confirmation of `prev`, one per engaged witness
    /// whose receipt the author holds, sorted by witness `kid`. May be empty; never required to
    /// be complete (§11.2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub receipts: Vec<String>,
    /// Author's pubky (z32).
    pub author: String,
    /// Author's chain key: the Grant `cnf` (z32).
    pub kid: String,
    /// Signer's own claim of when they acted, Unix milliseconds. Display only; never consensus.
    pub ts: u64,
    /// Rules kind, or a protocol kind (see [`Kind`]).
    pub kind: String,
    /// Rules-specific body, or a protocol body.
    pub body: Value,
    /// `BLAKE3(canonical_state)` after applying this link; mandatory (§6.1). Protocol kinds
    /// repeat the previous link's.
    pub state: String,
    /// Grant JWS: present on genesis (initiator's), `rekey` and `recover` (new key's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<String>,
}

impl Link {
    /// Parse `prev` as a hash; `None` for genesis.
    pub fn prev_hash(&self) -> Result<Option<Hash>, crate::Error> {
        if self.prev.is_empty() {
            Ok(None)
        } else {
            Hash::parse(&self.prev).map(Some)
        }
    }

    /// Classify the kind.
    pub fn kind(&self) -> Kind {
        Kind::parse(&self.kind)
    }
}

/// Protocol-level kinds never reach `Rules::apply` (§10); everything else is rules content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Commit-reveal nonce for seat randomisation (§6.6).
    Reveal,
    /// Key renewal signed by the old key (§6.7).
    Rekey,
    /// Key loss, signed by the new key, vetoable and delayed (§6.7).
    Recover,
    /// Chain end (§6.8); see [`CloseReason`] in the body.
    Close,
    /// Change to the engaged witness set (§11.2).
    Witnesses,
    /// Anything else: a rules kind.
    Rules,
}

impl Kind {
    /// Map the wire string to a kind.
    pub fn parse(s: &str) -> Self {
        match s {
            "reveal" => Kind::Reveal,
            "rekey" => Kind::Rekey,
            "recover" => Kind::Recover,
            "close" => Kind::Close,
            "witnesses" => Kind::Witnesses,
            _ => Kind::Rules,
        }
    }

    /// True for the protocol kinds, whose eligibility never consults the rules (§6.4).
    pub fn is_protocol(self) -> bool {
        !matches!(self, Kind::Rules)
    }
}

/// `body.reason` of a `close` link (§6.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloseReason {
    /// The rules say the state is terminal. Needs everyone.
    Finished,
    /// The parties stop by consent. Needs everyone.
    Agreed,
    /// The `subject` parties have stopped taking part. Needs every non-subject; judged
    /// outside rounds; final only when adjudicated.
    Abandoned,
}

/// Body of a `close` link.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloseBody {
    /// Why the chain ends.
    pub reason: CloseReason,
    /// Parties held to have walked away (`abandoned` only), by pubky.
    #[serde(default)]
    pub subject: Vec<String>,
    /// Unconfirmed proposals at this seq, as context (`abandoned` only).
    #[serde(default)]
    pub pending: Vec<String>,
}

/// Body of a `witnesses` link (§11.2). `add` and `remove` must not both be empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WitnessChange {
    /// Witnesses to seat from the next seq, each with its `engage.jws` embedded.
    #[serde(default)]
    pub add: Vec<WitnessAdd>,
    /// Engagement `kid`s to unseat from the next seq.
    #[serde(default)]
    pub remove: Vec<String>,
}

/// One added witness.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WitnessAdd {
    /// Witness pubky (z32).
    pub pubky: String,
    /// Engagement signing key (z32); must equal the embedded Grant's `cnf`.
    pub kid: String,
    /// The witness's `engage.jws`.
    pub engage: String,
}

/// Body of `rekey` and `recover` (§6.7).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyChangeBody {
    /// The new chain key (z32) — the new Grant's `cnf`.
    pub new_kid: String,
    /// `recover` only: the party's folder from the next seq (§7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `recover` only, optional, public forever.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Body of `reveal` (§6.6).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevealBody {
    /// The nonce whose BLAKE3 was committed in the genesis confirmation, base64url.
    pub nonce: String,
}
