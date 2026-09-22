//! Signing records with the Grant client key (§5.2).
//!
//! A [`Signer`] knows the party's identity, its chain key and app, and can produce a JWS under
//! that key. [`LocalSigner`] holds the keypair (a keyed app, or a test); the Pubky SDK signer
//! in `SessionSigner` (feature `pubky-sdk`) dispatches to a grant session, local or browser
//! delegated, through `GrantCredential::sign_jws` (§16.3).

use pubky_common::auth::grant::GrantClaims;
use pubky_common::auth::jws::{ClientId, GrantId, GRANT_JWS_TYP};
use pubky_common::capabilities::Capability;
use pubky_common::crypto::Keypair;
use serde::Serialize;

use pubky_mayfly::{typ, PROTOCOL_FOLDER};

use crate::portable::{MaybeSend, MaybeSync};
use crate::Error;

/// Who signs, as what, for which app.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait Signer: MaybeSend + MaybeSync {
    /// Identity (z32).
    fn pubky(&self) -> String;
    /// Chain key: the Grant `cnf` (z32).
    fn kid(&self) -> String;
    /// The app holding the seat.
    fn client_id(&self) -> String;
    /// The Grant JWS binding `kid` to `pubky`.
    fn grant_jws(&self) -> String;
    /// `/pub/<client_id>/mayfly/`.
    fn path(&self) -> String {
        format!("/pub/{}/{PROTOCOL_FOLDER}/", self.client_id())
    }
    /// Sign `payload` as a JWS with header `typ`.
    async fn sign_jws(&self, typ: &str, payload: serde_json::Value) -> Result<String, Error>;
}

/// Sign a record with `signer`, checking the `typ` is one of ours (§5.2).
pub async fn sign_record<T: Serialize + MaybeSync>(
    signer: &dyn Signer,
    typ_: &str,
    payload: &T,
) -> Result<Vec<u8>, Error> {
    if !typ::is_mayfly(typ_) {
        return Err(Error::Signer(format!(
            "typ {typ_:?} is not a Mayfly record type"
        )));
    }
    let value = serde_json::to_value(payload).map_err(|e| Error::Signer(e.to_string()))?;
    Ok(signer.sign_jws(typ_, value).await?.into_bytes())
}

/// A signer holding its client keypair and Grant: a keyed app, or a test.
#[derive(Clone)]
pub struct LocalSigner {
    identity: pubky_common::crypto::PublicKey,
    client: Keypair,
    client_id: String,
    grant: String,
}

impl LocalSigner {
    /// Mint a Grant under `identity` for a fresh client key on `client_id`, with write access
    /// to the app's folder, valid for `lifetime_secs` from `now_s`.
    pub fn mint(identity: &Keypair, client_id: &str, now_s: u64, lifetime_secs: u64) -> Self {
        let client = Keypair::random();
        let grant = GrantClaims {
            iss: identity.public_key(),
            client_id: ClientId::new(client_id).expect("client id"),
            caps: vec![Capability::read_write(format!("/pub/{client_id}/")).expect("cap")],
            cnf: client.public_key(),
            jti: GrantId::generate(),
            iat: now_s,
            exp: now_s + lifetime_secs,
        }
        .sign(identity, GRANT_JWS_TYP);
        Self {
            identity: identity.public_key(),
            client,
            client_id: client_id.to_string(),
            grant,
        }
    }

    /// From an existing client keypair and its Grant.
    pub fn from_grant(client: Keypair, grant: String) -> Result<Self, Error> {
        let claims = GrantClaims::decode(&grant).map_err(|e| Error::Signer(e.to_string()))?;
        if claims.cnf != client.public_key() {
            return Err(Error::Signer("Grant cnf is not this keypair".into()));
        }
        Ok(Self {
            identity: claims.iss,
            client,
            client_id: claims.client_id.to_string(),
            grant,
        })
    }

    /// The client keypair, for tests that need to sign as the old key after a key change.
    pub fn keypair(&self) -> &Keypair {
        &self.client
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Signer for LocalSigner {
    fn pubky(&self) -> String {
        self.identity.z32()
    }

    fn kid(&self) -> String {
        self.client.public_key().z32()
    }

    fn client_id(&self) -> String {
        self.client_id.clone()
    }

    fn grant_jws(&self) -> String {
        self.grant.clone()
    }

    async fn sign_jws(&self, typ_: &str, payload: serde_json::Value) -> Result<String, Error> {
        Ok(pubky_common::auth::jws::sign_jws(
            &self.client,
            typ_,
            &payload,
        ))
    }
}
