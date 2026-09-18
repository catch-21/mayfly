//! Reject payload (§6.3): refuse, pass, skip — or veto a recover.

use serde::{Deserialize, Serialize};

use crate::hash::ChainId;

/// "I vote for nothing in `round` of `seq`." Final within its round.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reject {
    /// Protocol version.
    pub v: u32,
    /// The chain.
    pub chain: ChainId,
    /// Sequence number.
    pub seq: u64,
    /// Round.
    pub round: u32,
    /// Hash of the refused link, or empty for a pass (by a designated proposer) or a skip (by
    /// anyone else, round ≥ 1, once per party per seq).
    #[serde(default)]
    pub link: String,
    /// Rejecter's chain key (z32).
    pub kid: String,
    /// Signer's own timestamp, Unix milliseconds; display only.
    pub ts: u64,
}

impl Reject {
    /// True for a pass or a skip: an empty reject.
    pub fn is_empty(&self) -> bool {
        self.link.is_empty()
    }
}
