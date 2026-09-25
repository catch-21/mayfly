//! Storage as the client sees it: read anyone's `/pub/` folder, write my own (§7).
//!
//! [`Store`] is the only I/O the chain client does. [`MemoryStore`] implements it over the
//! simulator's [`Storage`](pubky_mayfly::sim::Storage) so every flow can be exercised without a
//! network; `PubkyStore` (feature `pubky-sdk`) implements it over the Pubky SDK.

use std::sync::{Arc, Mutex};

use pubky_mayfly::sim::Storage;

use crate::portable::{MaybeSend, MaybeSync};
use crate::Error;

/// One file as listed: the owner's pubky, the absolute path, and — where the store can say —
/// the content's hash, so a client can tell an overwritten file from the copy it cached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// Owner (z32).
    pub owner: String,
    /// Absolute path, e.g. `/pub/chess.example/mayfly/chains/<id>/links/00000007-<h16>.jws`.
    pub path: String,
    /// `BLAKE3(bytes)` if known (a homeserver's `ETag` or SSE `content_hash`, §6).
    pub content_hash: Option<pubky_mayfly::hash::Hash>,
}

/// Read-anyone, write-me storage.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait Store: MaybeSend + MaybeSync {
    /// My pubky (z32); the owner every `put` writes as.
    fn me(&self) -> &str;

    /// Every file under `prefix` (an absolute folder path ending in `/`) in `owner`'s storage,
    /// with each file's content hash where the store can learn it cheaply enough to tell an
    /// overwritten mirror from a cached copy (§7).
    async fn list(&self, owner: &str, prefix: &str) -> Result<Vec<Listed>, Error>;

    /// Every file under `prefix`, names only: `content_hash` is `None`. For a reader that
    /// fetches what it has not seen by name and never compares a listing against a cache —
    /// the watchman, which receipts the bytes it fetches — this is one request per page of
    /// the listing instead of one per file. The default is [`Self::list`].
    async fn list_names(&self, owner: &str, prefix: &str) -> Result<Vec<Listed>, Error> {
        self.list(owner, prefix).await
    }

    /// Fetch one file; `None` if absent.
    async fn get(&self, owner: &str, path: &str) -> Result<Option<Vec<u8>>, Error>;

    /// Write one of my files (a homeserver `PUT`: overwrites).
    async fn put(&self, path: &str, bytes: Vec<u8>) -> Result<(), Error>;

    /// Delete one of my files.
    async fn delete(&self, path: &str) -> Result<(), Error>;
}

/// In-memory storage shared between simulated parties: the simulator's folder map, keyed by
/// `pubky://<owner><path>`, behind a mutex so several clients can share it.
#[derive(Clone)]
pub struct MemoryStore {
    me: String,
    shared: Arc<Mutex<Storage>>,
}

impl MemoryStore {
    /// A store for `me` over `shared`.
    pub fn new(me: impl Into<String>, shared: Arc<Mutex<Storage>>) -> Self {
        Self {
            me: me.into(),
            shared,
        }
    }

    /// A fresh shared storage.
    pub fn shared() -> Arc<Mutex<Storage>> {
        Arc::new(Mutex::new(Storage::default()))
    }

    /// The shared storage, for tests that want to tamper with it.
    pub fn storage(&self) -> Arc<Mutex<Storage>> {
        Arc::clone(&self.shared)
    }

    /// Split an absolute path into the simulator's `(folder key, file name)`. Folders in the
    /// simulator are the protocol folders (`pubky://<owner>/pub/<app>/mayfly/`) and file names
    /// are relative to them.
    fn locate(owner: &str, path: &str) -> Result<(String, String), Error> {
        // `/pub/<app>/<folder>/rest…`
        let mut parts = path.trim_start_matches('/').splitn(4, '/');
        let (Some(pub_), Some(app), Some(folder), Some(rest)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(Error::Store(format!(
                "path {path:?} is not under an app folder"
            )));
        };
        Ok((
            format!("pubky://{owner}/{pub_}/{app}/{folder}/"),
            rest.to_string(),
        ))
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Store for MemoryStore {
    fn me(&self) -> &str {
        &self.me
    }

    async fn list(&self, owner: &str, prefix: &str) -> Result<Vec<Listed>, Error> {
        let storage = self.shared.lock().expect("storage mutex");
        let mut out = Vec::new();
        let want = format!("pubky://{owner}{prefix}");
        for (folder, files) in &storage.folders {
            if !folder.starts_with(&format!("pubky://{owner}/")) {
                continue;
            }
            for (name, bytes) in files {
                let full = format!("{folder}{name}");
                if full.starts_with(&want) {
                    let path = full
                        .strip_prefix(&format!("pubky://{owner}"))
                        .expect("owner prefix")
                        .to_string();
                    out.push(Listed {
                        owner: owner.to_string(),
                        path,
                        content_hash: Some(pubky_mayfly::hash::Hash::of(bytes)),
                    });
                }
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    /// Names only, as a homeserver listing is: what a test of a names-only reader must see.
    async fn list_names(&self, owner: &str, prefix: &str) -> Result<Vec<Listed>, Error> {
        let mut out = self.list(owner, prefix).await?;
        for l in &mut out {
            l.content_hash = None;
        }
        Ok(out)
    }

    async fn get(&self, owner: &str, path: &str) -> Result<Option<Vec<u8>>, Error> {
        let (folder, name) = Self::locate(owner, path)?;
        let storage = self.shared.lock().expect("storage mutex");
        Ok(storage
            .folders
            .get(&folder)
            .and_then(|f| f.get(&name))
            .cloned())
    }

    async fn put(&self, path: &str, bytes: Vec<u8>) -> Result<(), Error> {
        let (folder, name) = Self::locate(&self.me, path)?;
        self.shared
            .lock()
            .expect("storage mutex")
            .put(&folder, &name, bytes);
        Ok(())
    }

    async fn delete(&self, path: &str) -> Result<(), Error> {
        let (folder, name) = Self::locate(&self.me, path)?;
        self.shared
            .lock()
            .expect("storage mutex")
            .delete(&folder, &name);
        Ok(())
    }
}
