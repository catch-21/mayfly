//! Reading a chain as anyone (§9, §14): list the initiator's folder, read the rules id from
//! genesis, verify with the rules the caller can supply for that id, then walk every folder
//! the verified head declares so a page can show each record with its checks. No seat, no
//! signer, no votes.
//!
//! The reader is the same for a page's viewer, a native explorer and a test; what differs is
//! which rules the caller knows, so the rules arrive through a resolver: given the id genesis
//! pins, return the rules to verify with, or nothing. A chain whose rules the caller lacks is
//! still listed and decoded, with [`Loaded::view_error`] saying why there is no verified view.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use pubky_mayfly::fold;
use pubky_mayfly::hash::{ChainId, Hash};
use pubky_mayfly::rules::Rules;

use crate::chain::verify_from;
use crate::layout::{parse_chain_url, Folder};
use crate::store::Store;
use crate::view::{chain_view, decode_record, ChainView, RecordView};
use crate::Error;

/// Which of the chain's declared folders a file was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FolderRole {
    /// The folder the chain URL names.
    Initiator,
    /// A seat's declared folder (§6.2).
    Seat,
    /// An engaged witness's `witness/<chain_id>/` (§11.2).
    Witness,
}

/// One folder the reader lists.
#[derive(Debug, Clone, Serialize)]
pub struct ReadFolder {
    /// Whose folder.
    pub role: FolderRole,
    /// The owner's pubky.
    pub owner: String,
    /// Absolute prefix listed, ending in `/`.
    pub prefix: String,
    /// Why the listing failed, if it did.
    pub error: Option<String>,
}

/// One listed file, decoded when it is a `.jws`.
#[derive(Debug, Clone, Serialize)]
pub struct ReadFile {
    /// The folder it was listed in.
    pub folder: ReadFolder,
    /// Absolute path on the owner's homeserver.
    pub path: String,
    /// Path relative to the folder prefix.
    pub relative: String,
    /// The store's content hash (the homeserver's `ETag`), base64url, when it reported one.
    pub content_hash: Option<String>,
    /// The decoded record; absent for files that are not `.jws` or could not be fetched.
    pub record: Option<RecordView>,
    /// Why the bytes could not be fetched, if they could not.
    pub fetch_error: Option<String>,
}

/// A chain as read from files: everything a page renders.
#[derive(Debug, Clone, Serialize)]
pub struct Loaded {
    /// Chain id.
    pub chain: String,
    /// The initiator folder's owner.
    pub owner: String,
    /// The initiator's protocol folder.
    pub folder: String,
    /// The canonical chain URL.
    pub url: String,
    /// Rules id read from genesis; `None` when genesis could not be found or read.
    pub rules: Option<String>,
    /// Whether the resolver supplied rules for it.
    pub rules_known: bool,
    /// The verified chain, when verification ran and succeeded.
    pub view: Option<ChainView>,
    /// Why there is no view: no genesis yet, unknown rules, a store error.
    pub view_error: Option<String>,
    /// Every folder walked, including ones whose listing failed.
    pub folders: Vec<ReadFolder>,
    /// Every file listed, decoded where possible.
    pub files: Vec<ReadFile>,
}

/// A chain URL, or a link to any record inside a chain folder, cut back to the chain: the id,
/// `(owner, protocol folder)`, and the canonical chain URL.
pub fn normalise_chain_url(input: &str) -> Result<(ChainId, (String, String), String), Error> {
    let bad = || Error::State(format!("{input:?} is not a chain or record URL"));
    let trimmed = input.trim();
    let at = trimmed.rfind("chains/").ok_or_else(bad)?;
    let after = &trimmed[at + "chains/".len()..];
    let id_len = after.find('/').unwrap_or(after.len());
    let base = &trimmed[..at + "chains/".len() + id_len];
    let url = format!("{base}/");
    let (chain, (owner, folder)) = parse_chain_url(&url)?;
    Ok((chain, (owner, folder), url))
}

/// Reads chains and remembers what it decoded, so a page polling every few seconds fetches
/// only files whose content hash changed.
#[derive(Debug, Default)]
pub struct Reader {
    /// Decoded records by `(owner, path, content hash)`.
    cache: BTreeMap<(String, String), (Hash, RecordView)>,
}

impl Reader {
    /// A reader with an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load a chain from a chain or record URL. `resolve` returns the rules for the id
    /// genesis pins, or `None` if the caller cannot run them.
    pub async fn load<R: Rules, S: Store>(
        &mut self,
        store: &S,
        input: &str,
        resolve: impl FnOnce(&str) -> Option<R>,
    ) -> Result<Loaded, Error> {
        let (chain, (owner, folder), url) = normalise_chain_url(input)?;
        let chain_dir = Folder::from_path(&folder).chain(&chain);
        let mut initiator = ReadFolder {
            role: FolderRole::Initiator,
            owner: owner.clone(),
            prefix: chain_dir.as_str().to_string(),
            error: None,
        };
        let mut files = self.walk(store, &mut initiator).await;

        // Rules detection: genesis carries the rules id in its body.
        let genesis = files
            .iter()
            .find(|f| f.relative.starts_with("links/00000000-") && f.relative.ends_with(".jws"));
        let rules_id: Option<String> = genesis
            .and_then(|f| f.record.as_ref())
            .and_then(|r| r.payload.get("body"))
            .and_then(|b| b.get("rules"))
            .and_then(Value::as_str)
            .map(str::to_string);

        let mut view = None;
        let mut view_error = None;
        let mut rules_known = false;
        if let Some(e) = &initiator.error {
            view_error = Some(format!("could not list the initiator's folder: {e}"));
        } else if let Some(g) = genesis {
            match (&g.record, &rules_id) {
                (None, _) => {
                    view_error = Some(format!(
                        "genesis could not be read: {}",
                        g.fetch_error
                            .clone()
                            .unwrap_or_else(|| "unknown error".into())
                    ));
                }
                (Some(r), None) => {
                    view_error = Some(match &r.error {
                        Some(e) => format!("genesis is not a readable record: {e}"),
                        None => "genesis carries no rules id in its body".into(),
                    });
                }
                (Some(_), Some(id)) => match resolve(id) {
                    None => {
                        view_error = Some(format!("no rules available for {id:?}"));
                    }
                    Some(rules) => {
                        rules_known = true;
                        match verify_from(&rules, store, &chain, (owner.clone(), folder.clone()))
                            .await
                        {
                            Ok(report) => {
                                view = Some(chain_view(&rules, &report.verdict, &report.suspects));
                            }
                            Err(e) => view_error = Some(e.to_string()),
                        }
                    }
                },
            }
        } else {
            view_error = Some(
                "no genesis yet: the initiator's chain folder has no links/00000000-<h16>.jws"
                    .into(),
            );
        }

        // Every seat's declared paths and every engaged witness's folder, deduplicated
        // against the initiator folder already listed.
        let mut folders = vec![initiator];
        if let Some(v) = &view {
            let mut more: Vec<ReadFolder> = Vec::new();
            let mut seen: std::collections::BTreeSet<(String, String)> =
                [(owner.clone(), chain_dir.as_str().to_string())].into();
            for seat in &v.seats {
                for p in &seat.paths {
                    let prefix = Folder::from_path(p).chain(&chain).as_str().to_string();
                    if seen.insert((seat.pubky.clone(), prefix.clone())) {
                        more.push(ReadFolder {
                            role: FolderRole::Seat,
                            owner: seat.pubky.clone(),
                            prefix,
                            error: None,
                        });
                    }
                }
            }
            for w in &v.engaged {
                let prefix = Folder::from_path(&w.path)
                    .witness(&chain)
                    .as_str()
                    .to_string();
                if seen.insert((w.pubky.clone(), prefix.clone())) {
                    more.push(ReadFolder {
                        role: FolderRole::Witness,
                        owner: w.pubky.clone(),
                        prefix,
                        error: None,
                    });
                }
            }
            for mut f in more {
                let walked = self.walk(store, &mut f).await;
                files.extend(walked);
                folders.push(f);
            }
        }

        Ok(Loaded {
            chain: chain.to_string(),
            owner,
            folder,
            url,
            rules: rules_id,
            rules_known,
            view,
            view_error,
            folders,
            files,
        })
    }

    /// List one folder and decode every `.jws` in it. A listing failure is recorded on the
    /// folder, not returned.
    async fn walk<S: Store>(&mut self, store: &S, folder: &mut ReadFolder) -> Vec<ReadFile> {
        let listed = match store.list(&folder.owner, &folder.prefix).await {
            Ok(l) => l,
            Err(e) => {
                folder.error = Some(e.to_string());
                return Vec::new();
            }
        };
        let mut out = Vec::with_capacity(listed.len());
        for entry in listed {
            let relative = entry
                .path
                .strip_prefix(folder.prefix.as_str())
                .unwrap_or(&entry.path)
                .to_string();
            let mut file = ReadFile {
                folder: folder.clone(),
                path: entry.path.clone(),
                relative,
                content_hash: entry.content_hash.map(|h| h.to_base64url()),
                record: None,
                fetch_error: None,
            };
            if entry.path.ends_with(".jws") {
                self.fetch(store, &folder.owner, entry.content_hash, &mut file)
                    .await;
            }
            out.push(file);
        }
        out
    }

    /// Fetch and decode one record, or take it from the cache when its hash has not changed.
    async fn fetch<S: Store>(
        &mut self,
        store: &S,
        owner: &str,
        etag: Option<Hash>,
        file: &mut ReadFile,
    ) {
        let key = (owner.to_string(), file.path.clone());
        if let (Some(h), Some((cached_hash, record))) = (etag.as_ref(), self.cache.get(&key)) {
            if cached_hash == h {
                file.record = Some(record.clone());
                return;
            }
        }
        match store.get(owner, &file.path).await {
            Err(e) => file.fetch_error = Some(e.to_string()),
            Ok(None) => {
                file.fetch_error = Some("file not found when fetched (listed, then gone)".into())
            }
            Ok(Some(bytes)) => {
                let record = decode_record(&bytes, &file.path, etag.as_ref());
                if let Some(h) = etag {
                    self.cache.insert(key, (h, record.clone()));
                }
                file.record = Some(record);
            }
        }
    }
}

/// Decode the genesis link's rules id from bytes, for callers that hold the file already.
pub fn rules_id_of_genesis(bytes: &[u8]) -> Option<String> {
    let link = fold::decode_link(bytes.to_vec()).ok()?;
    link.payload
        .body
        .get("rules")
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::normalise_chain_url;
    use pubky_mayfly::hash::ChainId;

    #[test]
    fn a_record_link_is_cut_back_to_its_chain() {
        let chain = ChainId::derive(b"g");
        let base = format!("pubky://alice/pub/list.example/mayfly/chains/{chain}/");
        for input in [
            base.clone(),
            base.trim_end_matches('/').to_string(),
            format!("{base}links/00000000-abcdefghijklmnop.jws"),
            format!("  {base}confirms/00000003-abcdefghijklmnop-kid.jws "),
        ] {
            let (id, (owner, folder), url) = normalise_chain_url(&input).unwrap();
            assert_eq!(id, chain);
            assert_eq!(owner, "alice");
            assert_eq!(folder, "/pub/list.example/mayfly/");
            assert_eq!(url, base);
        }
        assert!(normalise_chain_url("https://example.com/").is_err());
        assert!(normalise_chain_url("pubky://alice/pub/x/mayfly/chains/NOPE/").is_err());
    }
}
