//! Watchdog receipt payload (§11.3).

use serde::{Deserialize, Serialize};

use crate::hash::ChainId;

/// One receipt per observed record. A receipt binds `(chain, record hash, typ, seq, round, by,
/// observed_at)` under the watchdog's engagement key. It proves the watchdog claims to have
/// seen those bytes at that time — nothing about their validity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    /// Protocol version.
    pub v: u32,
    /// Always `"observed"`.
    pub kind: String,
    /// The chain.
    pub chain: ChainId,
    /// Hash of the observed record (base64url).
    pub record: String,
    /// The observed record's JWS `typ`; `mayfly-revoke` for a `keys/<kid>.revoked.jws`.
    pub typ: String,
    /// Sequence number of the observed record; absent for a revocation receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    /// Round of the observed record; absent for a revocation receipt or an abandoned-close
    /// confirmation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<u32>,
    /// `kid` of the observed record's signer (the revoked `kid`, for a revocation).
    pub by: String,
    /// The watchdog's engagement signing key (z32), as established in `engage.jws`.
    pub kid: String,
    /// When the watchdog saw it, Unix milliseconds.
    pub observed_at: u64,
    /// Where it saw it: the homeserver user and event cursor.
    pub source: Source,
    /// `false` if the record contradicts something already receipted (§11.3).
    pub consistent: bool,
}

/// Homeserver and event cursor a record was observed at.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// The homeserver user (pubky z32) whose storage the record was read from.
    pub pubky: String,
    /// The event-stream cursor at which it appeared.
    pub cursor: u64,
}
