//! Watchdog engagement payload (§11.2).

use serde::{Deserialize, Serialize};

use crate::hash::ChainId;

/// "I agree to watch this chain until `until`, and I was paid for it."
///
/// The witness key is established exactly as a party's: the embedded Grant is under the witness
/// pubky and its `cnf` is `kid`, which signs this and every receipt. Two engagements for one
/// pubky and chain: the later `until` governs going forward; an earlier `until` published later
/// is equivocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Engagement {
    /// Protocol version.
    pub v: u32,
    /// Always `"engage"`.
    pub kind: String,
    /// The chain.
    pub chain: ChainId,
    /// Engagement signing key (z32).
    pub kid: String,
    /// Grant JWS under the watchdog pubky with `cnf == kid`.
    pub grant: String,
    /// The watchdog app's folder under which `witness/<chain_id>/...` lives (§7).
    pub path: String,
    /// The parties it agreed to watch, by pubky.
    pub parties: Vec<String>,
    /// End of engagement, Unix seconds.
    pub until: u64,
    /// How it watches.
    pub policy: Policy,
    /// What it stores.
    pub service: Service,
    /// Proof of payment, optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment: Option<Payment>,
}

/// Polling and clock policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// Polling interval in milliseconds; also the tolerance for clock disagreement.
    pub poll_ms: u64,
    /// Clock source description, e.g. `"ntp"`.
    pub clock: String,
}

/// Service tier (§11.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Service {
    /// Signed receipts only.
    Receipts,
    /// Receipts plus byte-for-byte copies of every record.
    Mirror,
}

/// Lightning payment evidence. The invoice description must contain the `chain_id`, or the
/// engagement is rejected (§11.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Payment {
    /// Always `"lightning"` for now.
    pub method: String,
    /// BOLT11 invoice.
    pub invoice: String,
    /// Settlement preimage, hex.
    pub preimage: String,
    /// Amount.
    pub msat: u64,
}
