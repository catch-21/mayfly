//! Client errors as JavaScript `Error`s whose `name` is the variant, so an app can match on
//! `e.name === "AlreadyVoted"` and read `e.message` for the rest. `e.transient` is `true`
//! for errors that mean "the round is not ready for that yet" ([`Error::is_transient`]),
//! plus `Busy`; an app retries those and never shows them.

use pubky_mayfly_client::Error;
use wasm_bindgen::JsValue;

/// A JS `Error` named after the variant, flagged `transient` when the client says so.
pub fn js(e: Error) -> JsValue {
    let err = named(e.name(), &e.to_string());
    if e.is_transient() {
        mark_transient(&err);
    }
    err
}

fn mark_transient(err: &JsValue) {
    let _ = js_sys::Reflect::set(err, &JsValue::from_str("transient"), &JsValue::TRUE);
}

/// A JS `Error` with `name` and `message`. `Busy` is transient.
pub fn named(name: &str, message: &str) -> JsValue {
    let err = js_sys::Error::new(message);
    err.set_name(name);
    let v: JsValue = err.into();
    if name == "Busy" {
        mark_transient(&v);
    }
    v
}

/// Shorthand for an argument the caller got wrong.
pub fn input(message: impl AsRef<str>) -> JsValue {
    named("InvalidInput", message.as_ref())
}
