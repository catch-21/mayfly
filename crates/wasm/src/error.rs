//! Client errors as JavaScript `Error`s whose `name` is the variant, so an app can match on
//! `e.name === "AlreadyVoted"` and read `e.message` for the rest.

use pubky_mayfly_client::Error;
use wasm_bindgen::JsValue;

/// The variant name of a client error.
fn kind(e: &Error) -> &'static str {
    match e {
        Error::Store(_) => "Store",
        Error::Signer(_) => "Signer",
        Error::Core(_) => "Core",
        Error::Rules(_) => "Rules",
        Error::NotOpen => "NotOpen",
        Error::NoGenesis => "NoGenesis",
        Error::NotSeated => "NotSeated",
        Error::AlreadyVoted { .. } => "AlreadyVoted",
        Error::NotDesignated { .. } => "NotDesignated",
        Error::NoSuchCandidate => "NoSuchCandidate",
        Error::RoundDead => "RoundDead",
        Error::AwaitingWitnesses { .. } => "AwaitingWitnesses",
        Error::State(_) => "State",
        Error::Oversize { .. } => "Oversize",
    }
}

/// A JS `Error` named after the variant.
pub fn js(e: Error) -> JsValue {
    named(kind(&e), &e.to_string())
}

/// A JS `Error` with `name` and `message`.
pub fn named(name: &str, message: &str) -> JsValue {
    let err = js_sys::Error::new(message);
    err.set_name(name);
    err.into()
}

/// Shorthand for an argument the caller got wrong.
pub fn input(message: impl AsRef<str>) -> JsValue {
    named("InvalidInput", message.as_ref())
}
