//! Parties, seats and Grant verification (§5).
//!
//! The chain key is the Grant `cnf` key and the Grant JWS is the attestation. A verifier checks
//! a Grant offline: signature under `iss` (the party's pubky), `cnf == kid`, and write
//! capability on the folder the party declared. It never compares a record's `ts` with
//! `iat..exp` (§5.2); expiry is enforced by co-signers' clocks and by watchdog receipts.

use pubky_common::auth::grant::GrantClaims;
use pubky_common::crypto::PublicKey;

use crate::error::Error;

/// A party's seat as the fold currently understands it.
#[derive(Debug, Clone)]
pub struct Seat {
    /// The party's identity (pubky, z32).
    pub pubky: String,
    /// Established chain key (z32) — updated by committed `rekey` / `recover`.
    pub kid: String,
    /// The app that took the seat; `rekey` must keep it, `recover` may change it.
    pub client_id: String,
    /// Folders this party has declared, in seq order; the last is current (§7).
    pub paths: Vec<String>,
    /// Grant `exp` in Unix seconds, for the watchdog-receipt window check (§9.2 step 4).
    pub grant_exp: u64,
}

/// Verify a Grant JWS as the binding of `kid` to `pubky` with write access to `path` (§5.2).
///
/// Returns the decoded claims on success. Callers then record `client_id` and `exp`.
pub fn verify_grant(
    grant_jws: &str,
    pubky: &PublicKey,
    kid: &PublicKey,
    path: &str,
) -> Result<GrantClaims, Error> {
    let claims: GrantClaims =
        GrantClaims::decode(grant_jws).map_err(|e| Error::Grant(format!("decode: {e}")))?;
    if &claims.iss != pubky {
        return Err(Error::Grant("iss is not the party's pubky".into()));
    }
    if &claims.cnf != kid {
        return Err(Error::Grant("cnf is not the record's kid".into()));
    }
    verify_grant_signature(grant_jws, pubky)?;
    if !claims.caps.iter().any(|c| cap_covers_write(c, path)) {
        return Err(Error::Grant(format!(
            "no write capability on {path}: a Grant that could not have written the record is not evidence of intent to take part"
        )));
    }
    Ok(claims)
}

/// Verify the Grant's own Ed25519 signature under the identity key.
fn verify_grant_signature(grant_jws: &str, iss: &PublicKey) -> Result<(), Error> {
    // The Grant is a JWS with typ `pubky-grant`; reuse our splitter but bypass the typ
    // allow-list, since this is Pubky's record, not ours.
    let mut parts = grant_jws.split('.');
    let (h, p, s) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(h), Some(p), Some(s), None) => (h, p, s),
        _ => return Err(Error::Grant("malformed JWS".into())),
    };
    use base64::Engine;
    let sig_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|e| Error::Grant(format!("signature base64url: {e}")))?;
    let sig = pubky_common::crypto::Signature::from_slice(&sig_bytes)
        .map_err(|e| Error::Grant(format!("signature: {e}")))?;
    iss.verify(format!("{h}.{p}").as_bytes(), &sig)
        .map_err(|_| Error::Grant("identity signature does not verify".into()))
}

/// Does capability `cap` grant write access to `path` (the path or a parent)?
fn cap_covers_write(cap: &pubky_common::capabilities::Capability, path: &str) -> bool {
    let text = cap.to_string();
    let Some((scope, actions)) = text.rsplit_once(':') else {
        return false;
    };
    if !actions.contains('w') {
        return false;
    }
    let scope = scope.trim_end_matches('/');
    let path = path.trim_end_matches('/');
    path == scope || path.starts_with(&format!("{scope}/"))
}

/// Parse a z32 pubky string.
pub fn parse_z32(s: &str) -> Result<PublicKey, Error> {
    PublicKey::try_from(s).map_err(|e| Error::Grant(format!("bad z32 key {s:?}: {e}")))
}
