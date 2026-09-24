//! Rules chosen at runtime, with JSON state and body, so one exported `ChainClient` type
//! serves every rules module: the ones this package ships, found by id, and the ones an app
//! supplies as a JavaScript object ([`crate::jsrules`]).

use serde_json::Value;

use pubky_mayfly::genesis::Genesis;
use pubky_mayfly::record::{CloseBody, Confirmation, Link};
use pubky_mayfly::rules::{Nonce, Outcome, PartyIndex, Rules, RulesError, Status};
use pubky_mayfly_rules::list::List;

/// Every rules module a client can run.
#[derive(Debug, Clone)]
pub enum AnyRules {
    /// `list/1`.
    List(List),
    /// Rules supplied by the app as a JavaScript object.
    #[cfg(target_arch = "wasm32")]
    Js(crate::jsrules::JsRules),
}

impl AnyRules {
    /// The shipped rules for a pinned id, if any.
    pub fn by_id(id: &str) -> Option<Self> {
        match id {
            "list/1" => Some(Self::List(List)),
            _ => None,
        }
    }

    /// Every id this package ships.
    pub const IDS: &'static [&'static str] = &["list/1"];
}

/// JSON bytes with object keys sorted at every level: the default `canonical_state` for
/// JavaScript rules that do not supply one. The workspace's `serde_json` preserves insertion
/// order, so two honest verifiers building the same state in different key orders would
/// otherwise hash differently.
pub fn canonical_json(v: &Value) -> Vec<u8> {
    fn write(v: &Value, out: &mut Vec<u8>) {
        match v {
            Value::Object(map) => {
                out.push(b'{');
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                for (i, k) in keys.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    out.extend_from_slice(serde_json::to_string(k).expect("string").as_bytes());
                    out.push(b':');
                    write(&map[*k], out);
                }
                out.push(b'}');
            }
            Value::Array(items) => {
                out.push(b'[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    write(item, out);
                }
                out.push(b']');
            }
            other => {
                out.extend_from_slice(serde_json::to_string(other).expect("scalar").as_bytes())
            }
        }
    }
    let mut out = Vec::new();
    write(v, &mut out);
    out
}

fn conv<T: serde::Serialize>(v: T) -> Result<Value, RulesError> {
    serde_json::to_value(v).map_err(|e| RulesError(format!("state: {e}")))
}

fn state<T: serde::de::DeserializeOwned>(v: &Value) -> Result<T, RulesError> {
    serde_json::from_value(v.clone()).map_err(|e| RulesError(format!("state: {e}")))
}

impl Rules for AnyRules {
    type State = Value;
    type Body = Value;

    fn id(&self) -> &'static str {
        match self {
            Self::List(r) => r.id(),
            #[cfg(target_arch = "wasm32")]
            Self::Js(r) => r.id(),
        }
    }

    fn reference_hash(&self) -> &'static str {
        match self {
            Self::List(r) => r.reference_hash(),
            #[cfg(target_arch = "wasm32")]
            Self::Js(r) => r.reference_hash(),
        }
    }

    fn init(
        &self,
        genesis: &Genesis,
        confirmations: &[Confirmation],
        nonces: &[Nonce],
    ) -> Result<Value, RulesError> {
        match self {
            Self::List(r) => conv(r.init(genesis, confirmations, nonces)?),
            #[cfg(target_arch = "wasm32")]
            Self::Js(r) => r.init(genesis, confirmations, nonces),
        }
    }

    fn wants_reveals(&self, genesis: &Genesis) -> bool {
        match self {
            Self::List(r) => r.wants_reveals(genesis),
            #[cfg(target_arch = "wasm32")]
            Self::Js(r) => r.wants_reveals(genesis),
        }
    }

    fn obliged(&self, s: &Value) -> Vec<PartyIndex> {
        match self {
            Self::List(r) => state(s).map(|s| r.obliged(&s)).unwrap_or_default(),
            #[cfg(target_arch = "wasm32")]
            Self::Js(r) => r.obliged(s),
        }
    }

    fn may_append(&self, s: &Value, party: PartyIndex, kind: &str) -> bool {
        match self {
            Self::List(r) => state(s)
                .map(|s| r.may_append(&s, party, kind))
                .unwrap_or(false),
            #[cfg(target_arch = "wasm32")]
            Self::Js(r) => r.may_append(s, party, kind),
        }
    }

    fn apply(&self, s: &Value, link: &Link) -> Result<Value, RulesError> {
        match self {
            Self::List(r) => conv(r.apply(&state(s)?, link)?),
            #[cfg(target_arch = "wasm32")]
            Self::Js(r) => r.apply(s, link),
        }
    }

    fn status(&self, s: &Value) -> Status {
        match self {
            Self::List(r) => state(s).map(|s| r.status(&s)).unwrap_or(Status::Ongoing),
            #[cfg(target_arch = "wasm32")]
            Self::Js(r) => r.status(s),
        }
    }

    fn close(&self, s: &Value, close: &CloseBody) -> Result<Outcome, RulesError> {
        match self {
            Self::List(r) => r.close(&state(s)?, close),
            #[cfg(target_arch = "wasm32")]
            Self::Js(r) => r.close(s, close),
        }
    }

    fn canonical_state(&self, s: &Value) -> Vec<u8> {
        match self {
            Self::List(r) => state(s).map(|s| r.canonical_state(&s)).unwrap_or_default(),
            #[cfg(target_arch = "wasm32")]
            Self::Js(r) => r.canonical_state(s),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::canonical_json;
    use serde_json::json;

    #[test]
    fn canonical_json_sorts_keys_at_every_level() {
        let a = json!({ "b": [{ "z": 1, "a": 2 }], "a": "x" });
        let b = json!({ "a": "x", "b": [{ "a": 2, "z": 1 }] });
        assert_eq!(canonical_json(&a), canonical_json(&b));
        assert_eq!(
            String::from_utf8(canonical_json(&a)).unwrap(),
            r#"{"a":"x","b":[{"a":2,"z":1}]}"#
        );
    }
}
