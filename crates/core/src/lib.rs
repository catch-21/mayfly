//! Mayfly core.
//!
//! Everything here is pure: bytes in, verdicts out. No storage, no network, no clock. The
//! client crate supplies files and SSE events; the watchdog crate supplies receipts; this crate
//! decides what they mean.
//!
//! Module map, against the specification (`docs/MAYFLY.md`):
//!
//! | Module      | Spec     | Contents                                                        |
//! |-------------|----------|-----------------------------------------------------------------|
//! | `hash`      | §6, §6.5 | BLAKE3 record hashes, the three encodings, `chain_id`, `<h16>`  |
//! | `encoding`  | §6       | Strict JSON parsing rules                                       |
//! | `record`    | §6.1–6.3 | Link, confirmation, reject, receipt, engagement payloads + JWS  |
//! | `keys`      | §5       | Grant verification, seats, established keys                     |
//! | `genesis`   | §6.6     | Genesis body and safety-parameter checks                        |
//! | `rules`     | §10      | The `Rules` trait                                               |
//! | `vote`      | §6.4     | Rounds, votes, death, rotation, quorum certificates             |
//! | `close`     | §6.8     | Close states                                                    |
//! | `witness`   | §11      | Engagements, witness quorum, stopwatch                          |
//! | `fold`      | §9.2     | The verifier                                                    |
//! | `sim`       | §16.2    | N-party simulator and `tally/1` test rules for property tests   |

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod close;
pub mod encoding;
pub mod error;
pub mod fold;
pub mod genesis;
pub mod hash;
pub mod keys;
pub mod record;
pub mod rules;
pub mod sim;
pub mod vote;
pub mod witness;

pub use error::Error;
pub use hash::{ChainId, Hash};

/// Protocol version carried in every record's `v` field.
pub const PROTOCOL_VERSION: u32 = 1;

/// Sub-folder inside an app's `/pub/<client_id>/` under which all records live (§7, §17).
pub const PROTOCOL_FOLDER: &str = "mayfly";

/// JWS `typ` values (§5.2). The `pubky-*` namespace is reserved to Pubky auth and refused.
pub mod typ {
    /// A proposed link.
    pub const LINK: &str = "mayfly-link";
    /// A confirmation of a link in a round.
    pub const CONFIRM: &str = "mayfly-confirm";
    /// A reject, pass or skip in a round; or an old-key veto of a recover.
    pub const REJECT: &str = "mayfly-reject";
    /// A watchdog engagement or receipt.
    pub const WITNESS: &str = "mayfly-witness";
    /// A watchdog receipt of a `keys/<kid>.revoked.jws` file.
    pub const REVOKE: &str = "mayfly-revoke";

    /// Every `typ` this crate will sign or accept.
    pub const ALL: &[&str] = &[LINK, CONFIRM, REJECT, WITNESS, REVOKE];

    /// True if `typ` is one of ours and not in the reserved `pubky-*` namespace.
    pub fn is_mayfly(typ: &str) -> bool {
        ALL.contains(&typ) && !typ.starts_with("pubky-")
    }
}
