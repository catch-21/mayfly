//! [`Store`] and [`Signer`] over the Pubky SDK: public reads from any homeserver, writes and
//! signatures through a grant session (§16.3). Native only (feature `pubky-sdk`): in the
//! browser the SDK is a separate wasm module and JavaScript implements the traits instead.

use pubky::errors::RequestError;
use pubky::{GrantCredential, Pubky, PubkySession, PublicKey};
use pubky_common::auth::grant::GrantClaims;

use pubky_mayfly::hash::Hash;

use crate::signer::Signer;
use crate::store::{Listed, Store};
use crate::Error;

fn sdk(e: pubky::Error) -> Error {
    Error::Store(e.to_string())
}

/// A failure worth retrying: the transport (a dropped connection, a resolver hiccup) or the
/// server itself (5xx). A 4xx or a malformed response is final.
fn transient(e: &pubky::Error) -> bool {
    match e {
        pubky::Error::Request(RequestError::Transport(_)) => true,
        pubky::Error::Request(RequestError::Server { status, .. }) => status.is_server_error(),
        _ => false,
    }
}

/// Run `op` up to four times, backing off 200 ms, 600 ms, 1.8 s between transient failures.
/// Storage is remote and shared; a client that panics on one lost request is not a client.
async fn retrying<T, F, Fut>(mut op: F) -> Result<T, pubky::Error>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, pubky::Error>>,
{
    let mut delay = std::time::Duration::from_millis(200);
    let mut attempt = 0;
    loop {
        match op().await {
            Err(e) if transient(&e) && attempt < 3 => {
                attempt += 1;
                crate::time::sleep(delay).await;
                delay *= 3;
            }
            other => return other,
        }
    }
}

/// Storage over the Pubky SDK. Reads are public (anyone's `/pub/`); writes go through the
/// session, if there is one.
#[derive(Clone)]
pub struct PubkyStore {
    pubky: Pubky,
    session: Option<PubkySession>,
    me: String,
}

impl PubkyStore {
    /// A read-write store for the session's user.
    pub fn new(pubky: Pubky, session: PubkySession) -> Self {
        let me = session.public_key().z32();
        Self {
            pubky,
            session: Some(session),
            me,
        }
    }

    /// A read-only store: a verifier with no seat.
    pub fn read_only(pubky: Pubky) -> Self {
        Self {
            pubky,
            session: None,
            me: String::new(),
        }
    }

    /// The SDK facade, for event streams.
    pub fn pubky(&self) -> &Pubky {
        &self.pubky
    }

    /// The session, if any.
    pub fn session(&self) -> Option<&PubkySession> {
        self.session.as_ref()
    }
}

fn owner_key(owner: &str) -> Result<PublicKey, Error> {
    PublicKey::try_from(owner).map_err(|e| Error::Store(format!("owner {owner:?}: {e}")))
}

#[async_trait::async_trait]
impl Store for PubkyStore {
    fn me(&self) -> &str {
        &self.me
    }

    async fn list(&self, owner: &str, prefix: &str) -> Result<Vec<Listed>, Error> {
        let pk = owner_key(owner)?;
        let storage = self.pubky.public_storage();
        let mut out = self.list_names(owner, prefix).await?;
        // A listing carries no hashes, but every file's `ETag` is its BLAKE3 (§2, §6), so a
        // HEAD per entry tells a client which cached files were overwritten in place (§7) —
        // the tampered-mirror case — without fetching them. Done concurrently, in batches.
        for chunk in out.chunks_mut(8) {
            let stats = futures_util::future::join_all(
                chunk
                    .iter()
                    .map(|l| retrying(|| storage.stats((&pk, l.path.as_str())))),
            )
            .await;
            for (l, s) in chunk.iter_mut().zip(stats) {
                l.content_hash = s
                    .ok()
                    .flatten()
                    .and_then(|s| s.etag)
                    .and_then(|e| Hash::parse(&e).ok());
            }
        }
        Ok(out)
    }

    async fn list_names(&self, owner: &str, prefix: &str) -> Result<Vec<Listed>, Error> {
        let pk = owner_key(owner)?;
        let storage = self.pubky.public_storage();
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let page = retrying(|| async {
                let mut builder = storage.list((&pk, prefix))?.limit(1000);
                if let Some(c) = &cursor {
                    builder = builder.cursor(c);
                }
                builder.send().await
            })
            .await;
            let page = match page {
                Ok(page) => page,
                // A folder that does not exist yet lists as nothing.
                Err(pubky::Error::Request(RequestError::Server { status, .. }))
                    if status.as_u16() == 404 =>
                {
                    break;
                }
                Err(e) => return Err(sdk(e)),
            };
            let n = page.len();
            for r in page {
                out.push(Listed {
                    owner: owner.to_string(),
                    path: r.path.as_str().to_string(),
                    content_hash: None,
                });
            }
            if n < 1000 {
                break;
            }
            cursor = out.last().map(|l| format!("pubky://{}{}", l.owner, l.path));
        }
        Ok(out)
    }

    async fn get(&self, owner: &str, path: &str) -> Result<Option<Vec<u8>>, Error> {
        let pk = owner_key(owner)?;
        let storage = self.pubky.public_storage();
        match retrying(|| storage.get((&pk, path))).await {
            Ok(resp) => Ok(Some(
                resp.bytes()
                    .await
                    .map_err(|e| Error::Store(e.to_string()))?
                    .to_vec(),
            )),
            Err(pubky::Error::Request(RequestError::Server { status, .. }))
                if status.as_u16() == 404 =>
            {
                Ok(None)
            }
            Err(e) => Err(sdk(e)),
        }
    }

    async fn put(&self, path: &str, bytes: Vec<u8>) -> Result<(), Error> {
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| Error::Store("read-only store".into()))?;
        let storage = session.storage();
        retrying(|| storage.put(path, bytes.clone()))
            .await
            .map_err(sdk)?;
        Ok(())
    }

    async fn delete(&self, path: &str) -> Result<(), Error> {
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| Error::Store("read-only store".into()))?;
        let storage = session.storage();
        retrying(|| storage.delete(path)).await.map_err(sdk)?;
        Ok(())
    }
}

impl<R: pubky_mayfly::rules::Rules, K: Signer> crate::ChainClient<R, PubkyStore, K> {
    /// Wait for the next event under this chain's folder on any known party's homeserver
    /// (§8.2: the SSE watcher), or until `timeout`; falls back to polling if no stream can be
    /// opened. Returns whether a change was seen.
    pub async fn wait_for_event(&self, timeout: std::time::Duration) -> Result<bool, Error> {
        use futures_util::StreamExt;
        let pubky = self.store().pubky().clone();
        let mut streams = Vec::new();
        for (owner, path) in self.folders() {
            let Ok(pk) = PublicKey::try_from(owner.as_str()) else {
                continue;
            };
            let folder = crate::layout::Folder::from_path(&path);
            let prefix = folder.chain(self.chain()).as_str().to_string();
            match pubky
                .event_stream_for_user(&pk, None)
                .path(prefix)
                .live()
                .subscribe()
                .await
            {
                Ok(stream) => streams.push(stream),
                Err(_) => continue,
            }
        }
        if streams.is_empty() {
            return self.wait_for_change(timeout).await;
        }
        let mut merged = futures_util::stream::select_all(streams);
        match tokio::time::timeout(timeout, merged.next()).await {
            Ok(Some(Ok(_event))) => Ok(true),
            Ok(Some(Err(_))) | Ok(None) => self.wait_for_change(timeout).await,
            Err(_elapsed) => Ok(false),
        }
    }
}

/// A [`Signer`] over a Pubky grant session: the Grant client key signs records through
/// `GrantCredential::sign_jws`, local or browser delegated alike.
#[derive(Clone)]
pub struct SessionSigner {
    credential: GrantCredential,
    pubky: String,
    kid: String,
    client_id: String,
    grant: String,
}

impl SessionSigner {
    /// From a grant-backed session. Fails for cookie sessions, which hold no Grant.
    pub async fn from_session(session: &PubkySession) -> Result<Self, Error> {
        let view = session
            .as_grant()
            .ok_or_else(|| Error::Signer("not a grant session".into()))?;
        let credential = view.credential();
        let grant = credential.grant_jws().await;
        let claims = GrantClaims::decode(&grant).map_err(|e| Error::Signer(e.to_string()))?;
        Ok(Self {
            pubky: claims.iss.z32(),
            kid: credential.client_public_key().await.z32(),
            client_id: claims.client_id.to_string(),
            grant,
            credential,
        })
    }
}

#[async_trait::async_trait]
impl Signer for SessionSigner {
    fn pubky(&self) -> String {
        self.pubky.clone()
    }

    fn kid(&self) -> String {
        self.kid.clone()
    }

    fn client_id(&self) -> String {
        self.client_id.clone()
    }

    fn grant_jws(&self) -> String {
        self.grant.clone()
    }

    async fn sign_jws(&self, typ: &str, payload: serde_json::Value) -> Result<String, Error> {
        self.credential
            .sign_jws(typ, &payload)
            .await
            .map_err(|e| Error::Signer(e.to_string()))
    }
}
