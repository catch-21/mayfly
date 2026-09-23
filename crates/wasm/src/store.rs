//! A [`Store`] over a JavaScript object (§16.2.1).
//!
//! The app supplies an object with this shape, typically built over the Pubky SDK's
//! `session.storage` (writes) and `pubky.publicStorage` (reads):
//!
//! ```ts
//! interface Store {
//!   me: string;                                   // my pubky (z32), "" for a read-only store
//!   list(owner: string, prefix: string): Promise<Array<{ path: string; contentHash?: string }>>;
//!   get(owner: string, path: string): Promise<Uint8Array | null | undefined>;   // undefined: absent
//!   put(path: string, bytes: Uint8Array): Promise<void>;
//!   delete(path: string): Promise<void>;
//! }
//! ```
//!
//! `contentHash` is the file's `ETag` (any of the three spellings of §6); giving it lets the
//! client detect a tampered mirror without fetching every file.

use js_sys::{Array, Function, Promise, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use pubky_mayfly::hash::Hash;
use pubky_mayfly_client::{Error, Listed, Store};

/// A store implemented by a JavaScript object.
pub struct JsStore {
    obj: JsValue,
    me: String,
}

fn store_err(what: &str, e: JsValue) -> Error {
    Error::Store(format!("{what}: {}", describe(&e)))
}

/// A JS error or thrown value as text.
pub fn describe(e: &JsValue) -> String {
    if let Some(err) = e.dyn_ref::<js_sys::Error>() {
        return format!(
            "{}: {}",
            String::from(err.name()),
            String::from(err.message())
        );
    }
    e.as_string().unwrap_or_else(|| {
        js_sys::JSON::stringify(e)
            .map(String::from)
            .unwrap_or_default()
    })
}

/// Read a string property, or `""`.
pub fn string_prop(obj: &JsValue, name: &str) -> String {
    Reflect::get(obj, &JsValue::from_str(name))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default()
}

/// Call `obj.name(args…)`, awaiting the result if it is a promise.
pub async fn call(obj: &JsValue, name: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
    let f = Reflect::get(obj, &JsValue::from_str(name))?;
    let f = f
        .dyn_into::<Function>()
        .map_err(|_| JsValue::from_str(&format!("{name} is not a function")))?;
    let out = match args {
        [] => f.call0(obj)?,
        [a] => f.call1(obj, a)?,
        [a, b] => f.call2(obj, a, b)?,
        _ => f.apply(obj, &args.iter().cloned().collect::<Array>())?,
    };
    match out.dyn_into::<Promise>() {
        Ok(p) => JsFuture::from(p).await,
        Err(v) => Ok(v),
    }
}

impl JsStore {
    /// Wrap `obj`; reads `me` once.
    pub fn new(obj: JsValue) -> Result<Self, JsValue> {
        if !obj.is_object() {
            return Err(crate::error::input("store must be an object"));
        }
        let me = string_prop(&obj, "me");
        Ok(Self { obj, me })
    }
}

#[async_trait::async_trait(?Send)]
impl Store for JsStore {
    fn me(&self) -> &str {
        &self.me
    }

    async fn list(&self, owner: &str, prefix: &str) -> Result<Vec<Listed>, Error> {
        let out = call(
            &self.obj,
            "list",
            &[JsValue::from_str(owner), JsValue::from_str(prefix)],
        )
        .await
        .map_err(|e| store_err("list", e))?;
        let items = out
            .dyn_into::<Array>()
            .map_err(|_| Error::Store("list did not return an array".into()))?;
        let mut listed = Vec::with_capacity(items.length() as usize);
        for item in items.iter() {
            let path = match item.as_string() {
                Some(p) => p,
                None => string_prop(&item, "path"),
            };
            if path.is_empty() {
                return Err(Error::Store("list item without a path".into()));
            }
            let content_hash = Reflect::get(&item, &JsValue::from_str("contentHash"))
                .ok()
                .and_then(|v| v.as_string())
                .and_then(|s| Hash::parse(&s).ok());
            listed.push(Listed {
                owner: owner.to_string(),
                path,
                content_hash,
            });
        }
        Ok(listed)
    }

    async fn get(&self, owner: &str, path: &str) -> Result<Option<Vec<u8>>, Error> {
        let out = call(
            &self.obj,
            "get",
            &[JsValue::from_str(owner), JsValue::from_str(path)],
        )
        .await
        .map_err(|e| store_err("get", e))?;
        if out.is_null() || out.is_undefined() {
            return Ok(None);
        }
        let bytes = out
            .dyn_into::<Uint8Array>()
            .map_err(|_| Error::Store("get did not return a Uint8Array".into()))?;
        Ok(Some(bytes.to_vec()))
    }

    async fn put(&self, path: &str, bytes: Vec<u8>) -> Result<(), Error> {
        let arr = Uint8Array::from(bytes.as_slice());
        call(&self.obj, "put", &[JsValue::from_str(path), arr.into()])
            .await
            .map(|_| ())
            .map_err(|e| store_err("put", e))
    }

    async fn delete(&self, path: &str) -> Result<(), Error> {
        call(&self.obj, "delete", &[JsValue::from_str(path)])
            .await
            .map(|_| ())
            .map_err(|e| store_err("delete", e))
    }
}
