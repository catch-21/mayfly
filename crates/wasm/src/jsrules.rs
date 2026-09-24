//! A [`Rules`] implementation supplied by JavaScript (§10, §16.2.1).
//!
//! An app whose rules are not shipped in this module passes an object instead of a rules id:
//!
//! ```ts
//! interface RulesModule<State, Body> {
//!   id: string;                                  // "chess/1"
//!   referenceHash: string;                       // pinned in genesis (§6.6)
//!   init(genesis, confirmations, nonces: Uint8Array[]): State;   // throw to refuse genesis
//!   wantsReveals?(genesis): boolean;             // default false
//!   obliged?(state): number[];                   // whose clock runs; default []
//!   mayAppend(state, party: number, kind: string): boolean;
//!   apply(state, link): State;                   // pure; throw to refuse the link
//!   status?(state): null | { summary: string; winners?: number[] };  // finished outcome
//!   close(state, close): { summary: string; winners?: number[] };    // throw to refuse
//!   canonicalState?(state): Uint8Array | string; // default: JSON with sorted keys
//! }
//! ```
//!
//! Every method is synchronous: the fold runs `apply` on every party's machine and expects
//! the same bytes from `canonicalState`, so the rules must be a pure function of their
//! arguments (§10). A thrown JS error becomes a `RulesError` carrying its message. State and
//! body cross the boundary as plain JSON values.

use std::cell::RefCell;
use std::collections::HashSet;

use js_sys::{Function, Reflect, Uint8Array};
use serde_json::Value;
use wasm_bindgen::{JsCast, JsValue};

use pubky_mayfly::genesis::Genesis;
use pubky_mayfly::record::{CloseBody, Confirmation, Link};
use pubky_mayfly::rules::{Nonce, Outcome, PartyIndex, Rules, RulesError, Status};

use crate::error::input;
use crate::store::{describe, string_prop};

thread_local! {
    /// `Rules::id` returns `&'static str`; ids that arrive from JavaScript are interned once
    /// so that opening the same rules many times leaks one string, not one per client.
    static INTERNED: RefCell<HashSet<&'static str>> = RefCell::new(HashSet::new());
}

fn intern(s: String) -> &'static str {
    INTERNED.with(|set| {
        let mut set = set.borrow_mut();
        if let Some(existing) = set.get(s.as_str()) {
            return *existing;
        }
        let leaked: &'static str = Box::leak(s.into_boxed_str());
        set.insert(leaked);
        leaked
    })
}

/// Rules implemented by a JavaScript object.
#[derive(Clone)]
pub struct JsRules {
    obj: JsValue,
    id: &'static str,
    reference_hash: &'static str,
}

impl std::fmt::Debug for JsRules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsRules").field("id", &self.id).finish()
    }
}

fn to_js<T: serde::Serialize>(v: &T) -> Result<JsValue, RulesError> {
    let ser = serde_wasm_bindgen::Serializer::json_compatible();
    v.serialize(&ser)
        .map_err(|e| RulesError(format!("rules argument: {e}")))
}

fn from_js<T: serde::de::DeserializeOwned>(v: JsValue, what: &str) -> Result<T, RulesError> {
    serde_wasm_bindgen::from_value(v).map_err(|e| RulesError(format!("rules {what}: {e}")))
}

impl JsRules {
    /// Wrap a rules object; `id` and `referenceHash` are read once and must be non-empty.
    pub fn new(obj: JsValue) -> Result<Self, JsValue> {
        if !obj.is_object() {
            return Err(input("rules must be a rules id string or a rules object"));
        }
        let id = string_prop(&obj, "id");
        let reference_hash = string_prop(&obj, "referenceHash");
        if id.is_empty() || reference_hash.is_empty() {
            return Err(input(
                "a rules object needs non-empty `id` and `referenceHash` strings",
            ));
        }
        for required in ["init", "mayAppend", "apply", "close"] {
            let f = Reflect::get(&obj, &JsValue::from_str(required)).unwrap_or(JsValue::UNDEFINED);
            if !f.is_function() {
                return Err(input(format!(
                    "rules {id:?}: `{required}` must be a function"
                )));
            }
        }
        Ok(Self {
            obj,
            id: intern(id),
            reference_hash: intern(reference_hash),
        })
    }

    /// The function `name` on the rules object, if it has one.
    fn method(&self, name: &str) -> Option<Function> {
        Reflect::get(&self.obj, &JsValue::from_str(name))
            .ok()
            .and_then(|f| f.dyn_into::<Function>().ok())
    }

    /// Call `name(args…)` synchronously; a throw is a `RulesError` with the message.
    fn call(&self, name: &str, args: &[JsValue]) -> Result<JsValue, RulesError> {
        let f = self
            .method(name)
            .ok_or_else(|| RulesError(format!("rules {}: `{name}` is not a function", self.id)))?;
        let out = match args {
            [] => f.call0(&self.obj),
            [a] => f.call1(&self.obj, a),
            [a, b] => f.call2(&self.obj, a, b),
            [a, b, c] => f.call3(&self.obj, a, b, c),
            _ => unreachable!("rules methods take at most three arguments"),
        };
        out.map_err(|e| RulesError(format!("rules {}: {name}: {}", self.id, describe(&e))))
    }

    /// Call `name` if the object defines it; `None` when it does not.
    fn call_optional(&self, name: &str, args: &[JsValue]) -> Result<Option<JsValue>, RulesError> {
        if self.method(name).is_none() {
            return Ok(None);
        }
        self.call(name, args).map(Some)
    }
}

/// `{ summary, winners? }` from JavaScript.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OutcomeInput {
    summary: String,
    #[serde(default)]
    winners: Vec<PartyIndex>,
}

impl From<OutcomeInput> for Outcome {
    fn from(o: OutcomeInput) -> Self {
        Outcome {
            summary: o.summary,
            winners: o.winners,
        }
    }
}

impl Rules for JsRules {
    type State = Value;
    type Body = Value;

    fn id(&self) -> &'static str {
        self.id
    }

    fn reference_hash(&self) -> &'static str {
        self.reference_hash
    }

    fn init(
        &self,
        genesis: &Genesis,
        confirmations: &[Confirmation],
        nonces: &[Nonce],
    ) -> Result<Value, RulesError> {
        let nonces_js = js_sys::Array::new();
        for n in nonces {
            nonces_js.push(&Uint8Array::from(n.as_slice()).into());
        }
        let out = self.call(
            "init",
            &[to_js(genesis)?, to_js(&confirmations)?, nonces_js.into()],
        )?;
        from_js(out, "init returned a state that is not JSON")
    }

    fn wants_reveals(&self, genesis: &Genesis) -> bool {
        let Ok(g) = to_js(genesis) else {
            return false;
        };
        matches!(
            self.call_optional("wantsReveals", &[g]),
            Ok(Some(v)) if v.is_truthy()
        )
    }

    fn obliged(&self, state: &Value) -> Vec<PartyIndex> {
        let Ok(s) = to_js(state) else {
            return Vec::new();
        };
        match self.call_optional("obliged", &[s]) {
            Ok(Some(v)) => from_js(v, "obliged").unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    fn may_append(&self, state: &Value, party: PartyIndex, kind: &str) -> bool {
        let Ok(s) = to_js(state) else {
            return false;
        };
        self.call(
            "mayAppend",
            &[s, JsValue::from_f64(party as f64), JsValue::from_str(kind)],
        )
        .map(|v| v.is_truthy())
        .unwrap_or(false)
    }

    fn apply(&self, state: &Value, link: &Link) -> Result<Value, RulesError> {
        let out = self.call("apply", &[to_js(state)?, to_js(link)?])?;
        from_js(out, "apply returned a state that is not JSON")
    }

    fn status(&self, state: &Value) -> Status {
        let Ok(s) = to_js(state) else {
            return Status::Ongoing;
        };
        match self.call_optional("status", &[s]) {
            Ok(Some(v)) if !v.is_null() && !v.is_undefined() && !v.is_falsy() => {
                match from_js::<OutcomeInput>(v, "status") {
                    Ok(o) => Status::Finished(o.into()),
                    Err(_) => Status::Ongoing,
                }
            }
            _ => Status::Ongoing,
        }
    }

    fn close(&self, state: &Value, close: &CloseBody) -> Result<Outcome, RulesError> {
        let out = self.call("close", &[to_js(state)?, to_js(close)?])?;
        from_js::<OutcomeInput>(out, "close must return { summary, winners? }").map(Into::into)
    }

    fn canonical_state(&self, state: &Value) -> Vec<u8> {
        let js_state = match to_js(state) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        match self.call_optional("canonicalState", &[js_state]) {
            Ok(Some(v)) => {
                if let Some(s) = v.as_string() {
                    return s.into_bytes();
                }
                if let Ok(bytes) = v.dyn_into::<Uint8Array>() {
                    return bytes.to_vec();
                }
                Vec::new()
            }
            _ => crate::rules::canonical_json(state),
        }
    }
}
