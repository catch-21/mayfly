//! Storage layout (§7): every path a client reads or writes, derived from the party's declared
//! folder and a chain id. Filenames are never identity; the fold matches records by hash.

use pubky_mayfly::hash::{ChainId, Hash};
use pubky_mayfly::PROTOCOL_FOLDER;

/// A party's (or watchman's) protocol folder: `/pub/<client_id>/mayfly/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder(String);

impl Folder {
    /// From an app id: `/pub/<client_id>/mayfly/`.
    pub fn for_app(client_id: &str) -> Self {
        Self(format!("/pub/{client_id}/{PROTOCOL_FOLDER}/"))
    }

    /// From a declared `path` (§6.2, §6.7). Normalised to end in `/`.
    pub fn from_path(path: &str) -> Self {
        let mut p = path.to_string();
        if !p.ends_with('/') {
            p.push('/');
        }
        Self(p)
    }

    /// The folder as a path string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `keys/current.jws`.
    pub fn current_grant(&self) -> String {
        format!("{}keys/current.jws", self.0)
    }

    /// `keys/<kid>.revoked.jws`.
    pub fn revoked(&self, kid: &str) -> String {
        format!("{}keys/{kid}.revoked.jws", self.0)
    }

    /// `chains/<chain_id>/`.
    pub fn chain(&self, chain: &ChainId) -> ChainFolder {
        ChainFolder(format!("{}chains/{}/", self.0, chain))
    }

    /// `witness/<chain_id>/` — on a watchman's storage.
    pub fn witness(&self, chain: &ChainId) -> WitnessFolder {
        WitnessFolder(format!("{}witness/{}/", self.0, chain))
    }

    /// `index/active/<chain_id>`: the marker a party writes for a chain it is in (§7). Its
    /// body is the chain URL the party joined through; a watchman reads it as a request to
    /// watch (§11.2).
    pub fn active(&self, chain: &ChainId) -> String {
        format!("{}index/active/{}", self.0, chain)
    }

    /// `index/finished/<chain_id>`: where the marker moves when a `close` commits (§8.4).
    pub fn finished(&self, chain: &ChainId) -> String {
        format!("{}index/finished/{}", self.0, chain)
    }
}

/// Parse a chain URL — `pubky://<owner>/pub/<client_id>/mayfly/chains/<chain_id>/` (§9.1), the
/// form an invite carries and [`ChainFolder`] paths embed — into the id and the folder it
/// names, `(owner, protocol folder path)`.
pub fn parse_chain_url(url: &str) -> Result<(ChainId, (String, String)), crate::Error> {
    let bad = || crate::Error::State(format!("{url:?} is not a chain URL"));
    let rest = url.trim().strip_prefix("pubky://").ok_or_else(bad)?;
    let slash = rest.find('/').ok_or_else(bad)?;
    let (owner, path) = (&rest[..slash], &rest[slash..]);
    let at = path.rfind("chains/").ok_or_else(bad)?;
    let folder = &path[..at];
    let id = path[at + "chains/".len()..].trim_end_matches('/');
    if owner.is_empty()
        || Folder::from_path(folder).as_str() != folder
        || !folder.ends_with(&format!("/{PROTOCOL_FOLDER}/"))
    {
        return Err(bad());
    }
    let chain = ChainId::parse(id)?;
    Ok((chain, (owner.to_string(), folder.to_string())))
}

/// `…/chains/<chain_id>/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainFolder(String);

fn seq8(seq: u64) -> String {
    format!("{seq:08}")
}

impl ChainFolder {
    /// The folder.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `links/<seq>-<h16>.jws` for a link with the given hash.
    pub fn link(&self, seq: u64, hash: &Hash) -> String {
        format!("{}links/{}-{}.jws", self.0, seq8(seq), hash.h16())
    }

    /// `confirms/<seq>-<h16>.jws`: my own confirmation, named by the *referenced link's* hash.
    pub fn confirm(&self, seq: u64, link: &Hash) -> String {
        format!("{}confirms/{}-{}.jws", self.0, seq8(seq), link.h16())
    }

    /// `confirms/<seq>-<h16>-<kid>.jws`: a mirrored confirmation by another party (§7).
    pub fn mirrored_confirm(&self, seq: u64, link: &Hash, confirmer_kid: &str) -> String {
        format!(
            "{}confirms/{}-{}-{confirmer_kid}.jws",
            self.0,
            seq8(seq),
            link.h16()
        )
    }

    /// `rejects/<seq>-r<round>.jws`.
    pub fn reject(&self, seq: u64, round: u32) -> String {
        format!("{}rejects/{}-r{round}.jws", self.0, seq8(seq))
    }

    /// `receipts/<witness kid>/engage.jws`.
    pub fn mirrored_engagement(&self, witness_kid: &str) -> String {
        format!("{}receipts/{witness_kid}/engage.jws", self.0)
    }

    /// `receipts/<witness kid>/<seq>-<h16>.jws`, named by the receipt's own hash.
    pub fn mirrored_receipt(&self, witness_kid: &str, seq: u64, receipt: &Hash) -> String {
        format!(
            "{}receipts/{witness_kid}/{}-{}.jws",
            self.0,
            seq8(seq),
            receipt.h16()
        )
    }

    /// `receipts/<witness kid>/revoked-<kid>-<h16>.jws`.
    pub fn mirrored_revoke_receipt(&self, witness_kid: &str, kid: &str, receipt: &Hash) -> String {
        format!(
            "{}receipts/{witness_kid}/revoked-{kid}-{}.jws",
            self.0,
            receipt.h16()
        )
    }

    /// The listing prefixes a verifier reads.
    pub fn listing_prefixes(&self) -> [String; 4] {
        [
            format!("{}links/", self.0),
            format!("{}confirms/", self.0),
            format!("{}rejects/", self.0),
            format!("{}receipts/", self.0),
        ]
    }
}

/// `…/witness/<chain_id>/` on a watchman.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitnessFolder(String);

impl WitnessFolder {
    /// The folder.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Current `engage.jws`.
    pub fn engagement(&self) -> String {
        format!("{}engage.jws", self.0)
    }

    /// `engage/<kid>.jws`: every engagement ever held, kept forever.
    pub fn historic_engagement(&self, kid: &str) -> String {
        format!("{}engage/{kid}.jws", self.0)
    }

    /// `<seq>-<h16>.jws` receipt.
    pub fn receipt(&self, seq: u64, receipt: &Hash) -> String {
        format!("{}{}-{}.jws", self.0, seq8(seq), receipt.h16())
    }

    /// `revoked-<kid>-<h16>.jws`.
    pub fn revoke_receipt(&self, kid: &str, receipt: &Hash) -> String {
        format!("{}revoked-{kid}-{}.jws", self.0, receipt.h16())
    }

    /// `mirror/{links,confirms,rejects}/…` — `mirror` tier only.
    pub fn mirror(&self, sub: &str, filename: &str) -> String {
        format!("{}mirror/{sub}/{filename}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_follow_section_7() {
        let f = Folder::for_app("chess.example");
        assert_eq!(f.as_str(), "/pub/chess.example/mayfly/");
        let chain = ChainId::derive(b"g");
        let c = f.chain(&chain);
        let h = Hash::of(b"link");
        assert!(c.link(7, &h).starts_with(&format!(
            "/pub/chess.example/mayfly/chains/{chain}/links/00000007-"
        )));
        assert_eq!(
            c.reject(7, 1),
            format!("/pub/chess.example/mayfly/chains/{chain}/rejects/00000007-r1.jws")
        );
    }

    #[test]
    fn chain_urls_round_trip() {
        let chain = ChainId::derive(b"g");
        let url = format!("pubky://alice/pub/chess.example/mayfly/chains/{chain}/");
        let (id, folder) = parse_chain_url(&url).unwrap();
        assert_eq!(id, chain);
        assert_eq!(
            folder,
            ("alice".into(), "/pub/chess.example/mayfly/".into())
        );
        assert!(parse_chain_url("pubky://alice/pub/x/chains/NOPE/").is_err());
        assert!(parse_chain_url("https://example.com/").is_err());
    }

    #[test]
    fn declared_path_is_normalised() {
        assert_eq!(
            Folder::from_path("/pub/x.app/mayfly").as_str(),
            "/pub/x.app/mayfly/"
        );
    }
}
