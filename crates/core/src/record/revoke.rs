//! Revocation record (§5.3): `keys/<kid>.revoked.jws`, `typ: mayfly-revoke`.
//!
//! A `keys/` file, not a chain link: no `seq`, never a candidate, never a vote. Signed by
//! **another** key the same identity has attested — a current Grant's `cnf` under the same
//! pubky, with that Grant embedded — since the compromised key cannot be trusted to disown
//! itself. Its effect on the fold is through a watchdog receipt of it (§9.2 step 4): records by
//! the revoked `kid` that a witness quorum observed later are outside their window.

use serde::{Deserialize, Serialize};

/// "This identity disowns `revoked`."
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revocation {
    /// Protocol version.
    pub v: u32,
    /// Always `"revoked"`.
    pub kind: String,
    /// The disowned chain key (z32).
    pub revoked: String,
    /// The signing key (z32): the `cnf` of `grant`.
    pub by: String,
    /// Grant JWS under the same pubky whose `cnf` is `by`.
    pub grant: String,
    /// Signer's own timestamp, Unix milliseconds; display only.
    pub ts: u64,
}
