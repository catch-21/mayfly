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
/// tracking object nesting and the set of keys seen at each depth. Keys are compared after
/// JSON unescaping, so `"a"` and `"\u0061"` are the same key: two verifiers must not disagree
/// about a payload because one of them compared spellings and the other compared strings.
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
                        let key = unescape(literal)?;
                        if !f.keys.insert(key) {
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

/// The string a key literal denotes, with JSON escapes resolved. A literal `serde_json` cannot
/// read as a string is a malformed payload; it is refused here rather than left for the full
/// parse, so that the duplicate check never runs on a key it could not decode.
fn unescape(literal: &str) -> Result<String, Error> {
    serde_json::from_str::<String>(&format!("\"{literal}\""))
        .map_err(|e| Error::Payload(format!("key {literal:?}: {e}")))
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

    #[test]
    fn a_duplicate_key_is_a_duplicate_however_it_is_spelled() {
        assert!(parse_strict::<Value>(br#"{"a":1,"\u0061":2}"#).is_err());
        assert!(parse_strict::<Value>(br#"{"/":1,"\/":2}"#).is_err());
        assert!(parse_strict::<Value>(br#"{"\"":1,"\u0022":2}"#).is_err());
        assert!(parse_strict::<Value>(r#"{"\ud83d\ude00":1,"😀":2}"#.as_bytes()).is_err());
        assert!(parse_strict::<Value>(br#"{"a":1,"\u0062":2}"#).is_ok());
    }

    // ─── Properties ───────────────────────────────────────────────────────────────────────────

    use proptest::prelude::*;

    /// A document the generator controls, so the test knows every object's decoded keys before
    /// the document is spelled out.
    #[derive(Debug, Clone)]
    enum Doc {
        Null,
        Bool(bool),
        Num(u64),
        Str(String),
        Arr(Vec<Doc>),
        Obj(Vec<(String, Doc)>),
    }

    /// Short strings over a small alphabet, so repeated keys are common. The alphabet holds
    /// the two characters JSON must escape, the one it may escape short (`/`), a two-byte
    /// character, and one outside the BMP that a `\u` escape must write as a surrogate pair.
    fn text() -> impl Strategy<Value = String> {
        proptest::collection::vec(
            prop_oneof![
                Just('a'),
                Just('b'),
                Just('"'),
                Just('\\'),
                Just('/'),
                Just('é'),
                Just('😀'),
            ],
            0..=3,
        )
        .prop_map(String::from_iter)
    }

    fn doc() -> impl Strategy<Value = Doc> {
        let leaf = prop_oneof![
            Just(Doc::Null),
            any::<bool>().prop_map(Doc::Bool),
            (0..=MAX_SAFE_INTEGER).prop_map(Doc::Num),
            text().prop_map(Doc::Str),
        ];
        leaf.prop_recursive(3, 24, 4, |inner| {
            prop_oneof![
                proptest::collection::vec(inner.clone(), 0..4).prop_map(Doc::Arr),
                proptest::collection::vec((text(), inner), 0..4).prop_map(Doc::Obj),
            ]
        })
    }

    fn next(rng: &mut u64) -> u64 {
        *rng ^= *rng << 13;
        *rng ^= *rng >> 7;
        *rng ^= *rng << 17;
        *rng
    }

    /// Spell a string as a JSON literal, choosing per character between the raw character, a
    /// short escape where JSON has one, and a `\u` escape in lower or upper case hex.
    fn spell(s: &str, rng: &mut u64) -> String {
        let mut out = String::from("\"");
        for c in s.chars() {
            let choice = next(rng) % 4;
            let short = match c {
                '"' => Some("\\\""),
                '\\' => Some("\\\\"),
                '/' => Some("\\/"),
                _ => None,
            };
            match (choice, short) {
                (0 | 1, None) => out.push(c),
                (0 | 1, Some(_)) if c == '/' => out.push(c),
                (0 | 1, Some(e)) | (3, Some(e)) => out.push_str(e),
                (2, _) => {
                    let mut units = [0u16; 2];
                    for u in c.encode_utf16(&mut units) {
                        out.push_str(&format!("\\u{u:04x}"));
                    }
                }
                (_, None) => {
                    let mut units = [0u16; 2];
                    for u in c.encode_utf16(&mut units) {
                        out.push_str(&format!("\\u{u:04X}"));
                    }
                }
                _ => unreachable!(),
            }
        }
        out.push('"');
        out
    }

    /// Spell a document. `permute` writes each object's members in a different order.
    fn render(doc: &Doc, rng: &mut u64, permute: bool) -> String {
        match doc {
            Doc::Null => "null".into(),
            Doc::Bool(b) => b.to_string(),
            Doc::Num(n) => n.to_string(),
            Doc::Str(s) => spell(s, rng),
            Doc::Arr(items) => {
                let parts: Vec<String> = items.iter().map(|d| render(d, rng, permute)).collect();
                format!("[{}]", parts.join(","))
            }
            Doc::Obj(members) => {
                let mut order: Vec<usize> = (0..members.len()).collect();
                if permute {
                    for i in (1..order.len()).rev() {
                        let j = (next(rng) % (i as u64 + 1)) as usize;
                        order.swap(i, j);
                    }
                }
                let parts: Vec<String> = order
                    .into_iter()
                    .map(|i| {
                        let (k, v) = &members[i];
                        format!("{}:{}", spell(k, rng), render(v, rng, permute))
                    })
                    .collect();
                format!("{{{}}}", parts.join(","))
            }
        }
    }

    /// The reference: does any object repeat a key, comparing keys as strings?
    fn has_duplicate_key(doc: &Doc) -> bool {
        match doc {
            Doc::Arr(items) => items.iter().any(has_duplicate_key),
            Doc::Obj(members) => {
                let mut seen = std::collections::HashSet::new();
                members
                    .iter()
                    .any(|(k, v)| !seen.insert(k) || has_duplicate_key(v))
            }
            _ => false,
        }
    }

    /// The value a document denotes, for documents with no duplicate keys.
    fn to_value(doc: &Doc) -> Value {
        match doc {
            Doc::Null => Value::Null,
            Doc::Bool(b) => Value::Bool(*b),
            Doc::Num(n) => Value::from(*n),
            Doc::Str(s) => Value::String(s.clone()),
            Doc::Arr(items) => Value::Array(items.iter().map(to_value).collect()),
            Doc::Obj(members) => Value::Object(
                members
                    .iter()
                    .map(|(k, v)| (k.clone(), to_value(v)))
                    .collect(),
            ),
        }
    }

    proptest! {
        /// §6: the parser is the first thing to touch bytes anyone wrote. It refuses; it does
        /// not panic.
        #[test]
        fn parsing_never_panics(bytes in any::<Vec<u8>>()) {
            let _ = parse_strict::<Value>(&bytes);
        }

        /// The same, over text close enough to JSON to reach the scanner's deeper branches:
        /// unbalanced brackets, unterminated strings, stray escapes, and bare commas.
        #[test]
        fn parsing_near_json_never_panics(text in r#"[{}\[\]":,\\ /ua0-9\.\-]{0,40}"#) {
            let _ = parse_strict::<Value>(text.as_bytes());
        }

        /// §6 "Parsing rules": a payload is rejected exactly when some object repeats a key,
        /// comparing keys as the strings they decode to, however each is spelled. When it is
        /// accepted, it is the value the document denotes, and neither the spelling of its
        /// strings nor the order of its members changes that value.
        #[test]
        fn duplicate_keys_are_rejected_however_spelled(doc in doc(), seed_a in any::<u64>(), seed_b in any::<u64>()) {
            let duplicate = has_duplicate_key(&doc);
            let a = render(&doc, &mut (seed_a | 1), false);
            let b = render(&doc, &mut (seed_b | 1), true);
            let parsed_a = parse_strict::<Value>(a.as_bytes());
            let parsed_b = parse_strict::<Value>(b.as_bytes());
            prop_assert_eq!(parsed_a.is_err(), duplicate, "{}", a);
            prop_assert_eq!(parsed_b.is_err(), duplicate, "{}", b);
            if let (Ok(va), Ok(vb)) = (parsed_a, parsed_b) {
                let expected = to_value(&doc);
                prop_assert_eq!(&va, &expected, "{}", a);
                prop_assert_eq!(&vb, &expected, "{}", b);
            }
        }

        /// §6: every number is a non-negative integer no larger than `2^53 − 1`. Anything else
        /// — larger, negative, fractional, or in exponent form — is refused, whichever field
        /// carries it.
        #[test]
        fn numbers_outside_the_safe_integers_are_rejected(n in any::<u64>(), neg in 1..=i64::MAX, depth in 0usize..3) {
            let wrap = |lit: &str| {
                let mut s = lit.to_string();
                for _ in 0..depth {
                    s = format!("{{\"k\":[{s}]}}");
                }
                s
            };
            let accepts = |lit: String| parse_strict::<Value>(wrap(&lit).as_bytes()).is_ok();
            let integer = n.to_string();
            let negative = format!("-{neg}");
            let fraction = format!("{n}.5");
            let exponent = format!("{}e1", n % 1000);
            prop_assert_eq!(accepts(integer), n <= MAX_SAFE_INTEGER);
            prop_assert!(!accepts(negative));
            prop_assert!(!accepts(fraction));
            prop_assert!(!accepts(exponent));
        }
    }
}
