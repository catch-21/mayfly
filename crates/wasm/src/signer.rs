//! A [`Signer`] over a JavaScript object, and a keyed signer for Node and tests (§16.2.1).
//!
//! The app builds the object from a Pubky grant session:
//!
//! ```ts
//! const grant = session.grant!;
//! const signer: Signer = {
//!   pubky: session.info.publicKey.z32(),
//!   kid: (await grant.clientPublicKey()).z32(),
//!   clientId: session.info.clientId,
//!   grantJws: await grant.grantJws(),
//!   signJws: (typ, claims) => grant.signJws(typ, claims),
//! };
//! ```
//!
//! [`KeyedSigner`] holds its own identity and client keypair and mints its own Grant — a
//! keyed Node app, or a test that needs several parties without a homeserver. Its fields and
//! `signJws` have the same names, so `signer` above can be a `KeyedSigner` directly.

use wasm_bindgen::prelude::*;

use pubky_mayfly_client::{Error, LocalSigner, Signer};

use crate::store::{call, describe, string_prop};

/// A signer implemented by a JavaScript object.
pub struct JsSigner {
    obj: JsValue,
    pubky: String,
    kid: String,
    client_id: String,
    grant_jws: String,
}

impl JsSigner {
    /// Wrap `obj`; reads the four identity strings once.
    pub fn new(obj: JsValue) -> Result<Self, JsValue> {
        if !obj.is_object() {
            return Err(crate::error::input("signer must be an object"));
        }
        let s = Self {
            pubky: string_prop(&obj, "pubky"),
            kid: string_prop(&obj, "kid"),
            client_id: string_prop(&obj, "clientId"),
            grant_jws: string_prop(&obj, "grantJws"),
            obj,
        };
        for (name, v) in [
            ("pubky", &s.pubky),
            ("kid", &s.kid),
            ("clientId", &s.client_id),
            ("grantJws", &s.grant_jws),
        ] {
            if v.is_empty() {
                return Err(crate::error::input(format!(
                    "signer.{name} must be a non-empty string"
                )));
            }
        }
        Ok(s)
    }
}

#[async_trait::async_trait(?Send)]
impl Signer for JsSigner {
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
        self.grant_jws.clone()
    }

    async fn sign_jws(&self, typ: &str, payload: serde_json::Value) -> Result<String, Error> {
        let claims = serde_wasm_bindgen::to_value(&payload)
            .map_err(|e| Error::Signer(format!("claims: {e}")))?;
        let out = call(&self.obj, "signJws", &[JsValue::from_str(typ), claims])
            .await
            .map_err(|e| Error::Signer(format!("signJws: {}", describe(&e))))?;
        out.as_string()
            .ok_or_else(|| Error::Signer("signJws did not return a string".into()))
    }
}

/// A signer holding its own keys: identity, client keypair and a self-minted Grant.
///
/// For a keyed Node app, or tests. A browser app signs through its grant session instead.
#[wasm_bindgen]
pub struct KeyedSigner {
    inner: LocalSigner,
}

fn now_s() -> u64 {
    (js_sys::Date::now() / 1000.0) as u64
}

#[wasm_bindgen]
impl KeyedSigner {
    /// A fresh identity and client key, with a Grant for `clientId` valid for
    /// `lifetimeSecs` (default one year).
    #[wasm_bindgen(constructor)]
    pub fn new(client_id: &str, lifetime_secs: Option<u32>) -> KeyedSigner {
        let identity = pubky_common::crypto::Keypair::random();
        let lifetime = lifetime_secs.map(u64::from).unwrap_or(365 * 24 * 3600);
        Self {
            inner: LocalSigner::mint(&identity, client_id, now_s(), lifetime),
        }
    }

    /// From a 32-byte identity secret, minting a fresh client key and Grant for `clientId`.
    #[wasm_bindgen(js_name = "fromIdentitySecret")]
    pub fn from_identity_secret(
        secret: &[u8],
        client_id: &str,
        lifetime_secs: Option<u32>,
    ) -> Result<KeyedSigner, JsValue> {
        let secret: [u8; 32] = secret
            .try_into()
            .map_err(|_| crate::error::input("identity secret must be 32 bytes"))?;
        let identity = pubky_common::crypto::Keypair::from_secret(&secret);
        let lifetime = lifetime_secs.map(u64::from).unwrap_or(365 * 24 * 3600);
        Ok(Self {
            inner: LocalSigner::mint(&identity, client_id, now_s(), lifetime),
        })
    }

    /// Identity (z32).
    #[wasm_bindgen(getter)]
    pub fn pubky(&self) -> String {
        self.inner.pubky()
    }

    /// Chain key (z32).
    #[wasm_bindgen(getter)]
    pub fn kid(&self) -> String {
        self.inner.kid()
    }

    /// The app.
    #[wasm_bindgen(getter, js_name = "clientId")]
    pub fn client_id(&self) -> String {
        self.inner.client_id()
    }

    /// The Grant JWS.
    #[wasm_bindgen(getter, js_name = "grantJws")]
    pub fn grant_jws(&self) -> String {
        self.inner.grant_jws()
    }

    /// `/pub/<clientId>/mayfly/`.
    #[wasm_bindgen(getter)]
    pub fn path(&self) -> String {
        self.inner.path()
    }

    /// Sign `claims` as a JWS with header `typ`. Synchronous; `JsSigner` accepts either.
    #[wasm_bindgen(js_name = "signJws")]
    pub async fn sign_jws(&self, typ: String, claims: JsValue) -> Result<String, JsValue> {
        let payload: serde_json::Value = serde_wasm_bindgen::from_value(claims)
            .map_err(|e| crate::error::input(e.to_string()))?;
        self.inner
            .sign_jws(&typ, payload)
            .await
            .map_err(crate::error::js)
    }
}
