//! Strict JSON parsing rules (§6, "Parsing rules").
//!
//! Signed bytes are stored bytes, so there is no canonicalisation problem when *signing*. But
//! every verifier still parses the payload, and two honest verifiers must not read different
//! values from the same bytes. A payload is invalid if it has duplicate keys at any level; an
//! integer outside `0..=2^53-1`; a number with a fraction or exponent in an integer field; a
//! string that is not valid UTF-8; or an unknown top-level field for its `v`. Strings are
//! compared as bytes, never Unicode-normalised.

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::Error;

/// Largest integer every implementation (including JavaScript) agrees on.
pub const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// Parse a payload under the strict rules and then into `T`.
///
/// `T` should carry `#[serde(deny_unknown_fields)]` so that the "unknown top-level field"
/// rule is enforced by the type. The duplicate-key and integer-range rules are enforced here,
/// on the raw JSON, because serde's default behaviour is last-key-wins and lossy on numbers.
pub fn parse_strict<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, Error> {
    let text = std::str::from_utf8(bytes).map_err(|e| Error::Payload(format!("utf-8: {e}")))?;
    reject_duplicate_keys(text)?;
    let value: Value =
        serde_json::from_str(text).map_err(|e| Error::Payload(format!("json: {e}")))?;
    check_numbers(&value, "")?;
    serde_json::from_value(value).map_err(|e| Error::Payload(format!("shape: {e}")))
}

/// Walk the raw text once and refuse any object that repeats a key at the same level.
///
/// `serde_json` with `preserve_order` still silently overwrites duplicates, so this is done on
/// the token stream. It is deliberately simple: a small hand-rolled scanner over the string,
/// tracking object nesting and the set of keys seen at each depth.
fn reject_duplicate_keys(text: &str) -> Result<(), Error> {
    #[derive(Default)]
    struct Frame {
        keys: std::collections::HashSet<String>,
        expecting_key: bool,
        is_object: bool,
    }

    let mut stack: Vec<Frame> = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '{' => stack.push(Frame {
                keys: Default::default(),
                expecting_key: true,
                is_object: true,
            }),
            '[' => stack.push(Frame {
                keys: Default::default(),
                expecting_key: false,
                is_object: false,
            }),
            '}' | ']' => {
                stack.pop();
            }
            ',' => {
                if let Some(f) = stack.last_mut() {
                    if f.is_object {
                        f.expecting_key = true;
                    }
                }
            }
            '"' => {
                // Read the string literal, honouring escapes.
                let start = i + 1;
                let mut end = start;
                let mut escaped = false;
                for (j, d) in chars.by_ref() {
                    if escaped {
                        escaped = false;
                        continue;
                    }
                    match d {
                        '\\' => escaped = true,
                        '"' => {
                            end = j;
                            break;
                        }
                        _ => {}
                    }
                }
                let literal = &text[start..end];
                if let Some(f) = stack.last_mut() {
                    if f.is_object && f.expecting_key {
                        if !f.keys.insert(literal.to_string()) {
                            return Err(Error::Payload(format!("duplicate key {literal:?}")));
                        }
                        f.expecting_key = false;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Every number must be a non-negative integer within `MAX_SAFE_INTEGER`. The protocol has no
/// fractional or negative fields; a payload that carries one is malformed.
fn check_numbers(value: &Value, path: &str) -> Result<(), Error> {
    match value {
        Value::Number(n) => match n.as_u64() {
            Some(u) if u <= MAX_SAFE_INTEGER => Ok(()),
            Some(_) => Err(Error::Payload(format!("{path}: integer above 2^53-1"))),
            None => Err(Error::Payload(format!(
                "{path}: not a non-negative integer"
            ))),
        },
        Value::Array(items) => items
            .iter()
            .enumerate()
            .try_for_each(|(i, v)| check_numbers(v, &format!("{path}[{i}]"))),
        Value::Object(map) => map
            .iter()
            .try_for_each(|(k, v)| check_numbers(v, &format!("{path}.{k}"))),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        #[allow(dead_code)]
        seq: u64,
    }

    #[test]
    fn duplicate_keys_rejected() {
        assert!(parse_strict::<P>(br#"{"seq":1,"seq":2}"#).is_err());
    }

    #[test]
    fn nested_duplicate_rejected_but_same_key_in_sibling_objects_fine() {
        assert!(parse_strict::<Value>(br#"{"a":{"x":1,"x":2}}"#).is_err());
        assert!(parse_strict::<Value>(br#"{"a":{"x":1},"b":{"x":2}}"#).is_ok());
    }

    #[test]
    fn big_and_fractional_numbers_rejected() {
        assert!(parse_strict::<P>(br#"{"seq":9007199254740992}"#).is_err());
        assert!(parse_strict::<P>(br#"{"seq":1.5}"#).is_err());
        assert!(parse_strict::<P>(br#"{"seq":-1}"#).is_err());
        assert!(parse_strict::<P>(br#"{"seq":9007199254740991}"#).is_ok());
    }

    #[test]
    fn unknown_field_rejected() {
        assert!(parse_strict::<P>(br#"{"seq":1,"extra":true}"#).is_err());
    }
}
