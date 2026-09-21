//! Record hashes and the three spellings the homeserver and payloads use for them (§6).
//!
//! Same 32 BLAKE3 bytes everywhere. The homeserver writes them as quoted, padded standard
//! Base64 in `ETag` and unpadded standard Base64 in SSE `content_hash`; payloads write them as
//! unpadded base64url; filenames use a 16-character Crockford Base32 prefix. Implementations
//! decode to bytes and compare bytes. Never compare strings.

use std::fmt;

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::error::Error;

/// A 32-byte BLAKE3 hash of a record's bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hash([u8; 32]);

impl Hash {
    /// Hash the stored bytes of a record (the whole JWS compact string).
    pub fn of(bytes: &[u8]) -> Self {
        Self(*pubky_common::crypto::hash(bytes).as_bytes())
    }

    /// The raw bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Construct from raw bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Payload spelling: unpadded base64url.
    pub fn to_base64url(&self) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.0)
    }

    /// `ETag` spelling as the homeserver sends it: quoted, padded, standard Base64.
    pub fn to_etag(&self) -> String {
        format!(
            "\"{}\"",
            base64::engine::general_purpose::STANDARD.encode(self.0)
        )
    }

    /// SSE `content_hash` spelling: unpadded standard Base64.
    pub fn to_content_hash(&self) -> String {
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(self.0)
    }

    /// First sixteen Crockford Base32 characters (80 bits), used in filenames (§7).
    /// A filename is never identity; this exists so competing proposals do not collide.
    pub fn h16(&self) -> String {
        let full = base32::encode(base32::Alphabet::Crockford, &self.0);
        full[..16].to_string()
    }

    /// Decode any of the accepted spellings: quoted or bare, padded or not, standard or url.
    /// Fails closed on anything that does not decode to exactly 32 bytes.
    pub fn parse(s: &str) -> Result<Self, Error> {
        let s = s.trim().trim_matches('"');
        use base64::engine::general_purpose as gp;
        let engines: [gp::GeneralPurpose; 4] = [
            gp::URL_SAFE_NO_PAD,
            gp::URL_SAFE,
            gp::STANDARD_NO_PAD,
            gp::STANDARD,
        ];
        for engine in &engines {
            if let Ok(bytes) = engine.decode(s) {
                if bytes.len() == 32 {
                    let mut out = [0u8; 32];
                    out.copy_from_slice(&bytes);
                    return Ok(Self(out));
                }
            }
        }
        Err(Error::Hash(format!("not a 32-byte hash: {s:?}")))
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash({})", self.to_base64url())
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_base64url())
    }
}

impl Serialize for Hash {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_base64url())
    }
}

impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Hash::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// `chain_id = crockford_base32(BLAKE3(genesis bytes)[0..16])`, 26 characters (§6.5).
///
/// The genesis link cannot carry its own id (the id is derived from its bytes), so its `chain`
/// is the empty id; every later record carries the derived one.
#[derive(Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChainId(String);

impl ChainId {
    /// Derive from the genesis record's bytes.
    pub fn derive(genesis_bytes: &[u8]) -> Self {
        let h = Hash::of(genesis_bytes);
        Self(base32::encode(
            base32::Alphabet::Crockford,
            &h.as_bytes()[..16],
        ))
    }

    /// The empty id a genesis link carries.
    pub fn none() -> Self {
        Self(String::new())
    }

    /// Parse an id from a file name or URL segment: 26 Crockford Base32 characters.
    pub fn parse(s: &str) -> Result<Self, Error> {
        let ok = s.len() == 26
            && s.chars()
                .all(|c| c.is_ascii_digit() || (c.is_ascii_uppercase() && !"ILOU".contains(c)));
        if ok {
            Ok(Self(s.to_string()))
        } else {
            Err(Error::Payload(format!(
                "chain id {s:?} is not 26 Crockford characters"
            )))
        }
    }

    /// True for the genesis placeholder.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The 26 Crockford characters.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The bytes used as the rotation preimage prefix (§6.4): the ASCII of the id.
    pub fn ascii(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl fmt::Debug for ChainId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ChainId({})", self.0)
    }
}

impl fmt::Display for ChainId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_spellings_decode_to_the_same_bytes() {
        let h = Hash::of(b"hello");
        for s in [h.to_base64url(), h.to_etag(), h.to_content_hash()] {
            assert_eq!(Hash::parse(&s).unwrap(), h, "{s}");
        }
    }

    #[test]
    fn chain_id_is_26_crockford_chars() {
        let id = ChainId::derive(b"genesis");
        assert_eq!(id.as_str().len(), 26);
        assert!(id
            .as_str()
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase()));
    }

    #[test]
    fn chain_id_parses_what_it_derives() {
        let id = ChainId::derive(b"genesis");
        assert_eq!(ChainId::parse(id.as_str()).unwrap(), id);
        assert!(ChainId::parse("not-a-chain-id").is_err());
        assert!(ChainId::parse(&id.as_str().to_lowercase()).is_err());
    }

    #[test]
    fn h16_is_sixteen_chars() {
        assert_eq!(Hash::of(b"x").h16().len(), 16);
    }

    #[test]
    fn wrong_length_fails_closed() {
        assert!(Hash::parse("AAAA").is_err());
    }
}
