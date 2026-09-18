//! Confirmation payload (§6.2).

use serde::{Deserialize, Serialize};

use crate::hash::ChainId;

/// "I have validated this link and vote for it in `round` of `seq`." Final within its round.
///
/// Confirmations of an `abandoned` close carry no `round` (§6.8): they are judged outside
/// rounds, hence `Option`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Confirmation {
    /// Protocol version.
    pub v: u32,
    /// The chain.
    pub chain: ChainId,
    /// Sequence number of the link confirmed.
    pub seq: u64,
    /// Round of the link confirmed; absent for an abandoned close.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<u32>,
    /// Hash of the link confirmed (base64url).
    pub link: String,
    /// Confirmer's chain key (z32).
    pub kid: String,
    /// Signer's own timestamp, Unix milliseconds; display only.
    pub ts: u64,
    /// The confirmer's *own* canonical state hash after the link. A mismatch with the link's is
    /// a rules-divergence anomaly, never an invalid vote (§6.2).
    pub state: String,
    /// Genesis confirmations only: the confirmer's Grant JWS — their key attestation (§5.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<String>,
    /// Genesis confirmations only: the folder under which this party's records live (§7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Genesis confirmations only, when the rules want randomness: `BLAKE3(nonce)` (§6.6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}
