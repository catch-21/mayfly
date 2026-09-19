//! Records: the signed things that live in files (§6).
//!
//! Every record is a JWS compact string stored as the raw file body, so `Hash::of(bytes)` is
//! both the record's identity and the homeserver's `ETag`. [`Signed`] pairs the bytes with the
//! decoded header and payload; constructing one performs the split, the `typ` check and the
//! strict parse, but **not** the signature check, because which key to verify under is a
//! question for the fold (a party's established key as of that seq, or a witness's engagement).

mod confirm;
mod engage;
mod link;
mod receipt;
mod reject;
mod revoke;

pub use confirm::Confirmation;
pub use engage::{Engagement, Payment, Policy, Service};
pub use link::{
    CloseBody, CloseReason, KeyChangeBody, Kind, Link, RevealBody, WitnessAdd, WitnessChange,
};
pub use receipt::{Receipt, Source};
pub use reject::Reject;
pub use revoke::Revocation;

use base64::Engine;
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::encoding::parse_strict;
use crate::error::Error;
use crate::hash::Hash;
use crate::typ;

/// Decoded JWS header. Only `alg` and `typ` are permitted.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    /// Always `EdDSA`.
    pub alg: String,
    /// One of [`crate::typ`].
    pub typ: String,
}

/// A record as found in a file: its exact bytes, its hash, and its decoded parts.
#[derive(Debug, Clone)]
pub struct Signed<T> {
    /// The stored bytes — the JWS compact string. Never re-serialise; these are the identity.
    pub bytes: Vec<u8>,
    /// `BLAKE3(bytes)`.
    pub hash: Hash,
    /// Decoded header.
    pub header: Header,
    /// Decoded, strictly parsed payload.
    pub payload: T,
    /// The Ed25519 signature bytes.
    pub signature: Vec<u8>,
    /// `base64url(header).base64url(payload)` — what the signature is over.
    pub signing_input: String,
}

impl<T: DeserializeOwned> Signed<T> {
    /// Split, decode, check `typ`, and strictly parse. Does not verify the signature.
    pub fn decode(bytes: Vec<u8>, expected_typ: &str) -> Result<Self, Error> {
        let text = std::str::from_utf8(&bytes)
            .map_err(|e| Error::Jws(format!("compact string is not utf-8: {e}")))?;
        let mut parts = text.split('.');
        let (h, p, s) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(h), Some(p), Some(s), None) => (h, p, s),
            _ => return Err(Error::Jws("expected three dot-separated parts".into())),
        };
        let url = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header_bytes = url
            .decode(h)
            .map_err(|e| Error::Jws(format!("header base64url: {e}")))?;
        let header: Header = parse_strict(&header_bytes)?;
        if header.alg != "EdDSA" {
            return Err(Error::Jws(format!("alg {:?}", header.alg)));
        }
        if !typ::is_mayfly(&header.typ) || header.typ != expected_typ {
            return Err(Error::Typ(header.typ));
        }
        let payload_bytes = url
            .decode(p)
            .map_err(|e| Error::Jws(format!("payload base64url: {e}")))?;
        let payload: T = parse_strict(&payload_bytes)?;
        let signature = url
            .decode(s)
            .map_err(|e| Error::Jws(format!("signature base64url: {e}")))?;
        if signature.len() != 64 {
            return Err(Error::Jws("signature is not 64 bytes".into()));
        }
        let hash = Hash::of(&bytes);
        Ok(Self {
            signing_input: format!("{h}.{p}"),
            bytes,
            hash,
            header,
            payload,
            signature,
        })
    }

    /// Verify the Ed25519 signature under `key`.
    pub fn verify(&self, key: &pubky_common::crypto::PublicKey) -> Result<(), Error> {
        let sig = pubky_common::crypto::Signature::from_slice(&self.signature)
            .map_err(|e| Error::Jws(format!("signature: {e}")))?;
        key.verify(self.signing_input.as_bytes(), &sig)
            .map_err(|_| Error::Jws("signature does not verify".into()))
    }
}

/// Sign a payload into a record. Used by clients, watchdogs and the simulator; the verifier
/// never signs. `typ` must be one of [`crate::typ`] — the `pubky-*` namespace is refused.
pub fn sign<T: serde::Serialize>(
    keypair: &pubky_common::crypto::Keypair,
    typ: &str,
    payload: &T,
) -> Result<Vec<u8>, Error> {
    if !typ::is_mayfly(typ) {
        return Err(Error::Typ(typ.to_string()));
    }
    Ok(pubky_common::auth::jws::sign_jws(keypair, typ, payload).into_bytes())
}
