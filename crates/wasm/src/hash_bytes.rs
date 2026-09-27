//! BLAKE3 for bytes an app names itself (§6).
//!
//! Record, state and chain hashes are already on the view. A rules module written in
//! JavaScript cannot call [`Hash::of`](pubky_mayfly::hash::Hash::of), so a page that names a
//! file or a clause would otherwise pick its own hash and its own spelling.

use pubky_mayfly::hash::Hash;

/// `Hash::of(bytes)` in the payload spelling: unpadded base64url.
pub fn hash_bytes(bytes: &[u8]) -> String {
    Hash::of(bytes).to_base64url()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_matches_hash_of() {
        assert_eq!(hash_bytes(b"hello"), Hash::of(b"hello").to_base64url());
    }
}
