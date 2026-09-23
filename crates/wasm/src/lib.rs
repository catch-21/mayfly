//! Mayfly for JavaScript (§16.2.1, phase C).
//!
//! One wasm module carries the chain client, the verifier and the views; storage and signing
//! come from the calling JavaScript through duck-typed objects (see [`store`] and [`signer`]),
//! so the Pubky SDK's own npm package stays the only network code and a browser session never
//! has to cross into this module.
//!
//! Exports:
//!
//! - `ChainClient` — `create`, `openUrl`, `sync`, `join`, `act`, `proposeBody`, `confirm`,
//!   `reject`, `repropose`, `pass`, `skip`, `proposeClose`, `proposeAbandoned`,
//!   `confirmAbandoned`, `proposeReveal`, `mirror`, `waitForChange`, `state`, `view`, …
//! - `verifyFrom(rulesId, store, chainUrl)` — the chain view as a bystander sees it.
//! - `decodeRecord(bytes, name, etag?)` — one file for the evidence panel.
//! - `parseChainUrl`, `chainUrl`, `rulesIds`.
//! - `KeyedSigner` — a self-contained signer for Node apps and tests.
//!
//! The native build of this crate exists so the workspace's tests and clippy cover the
//! rules adapter; the JS-facing modules compile for `wasm32` only.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod error;
pub mod rules;

#[cfg(target_arch = "wasm32")]
pub mod client;
#[cfg(target_arch = "wasm32")]
pub mod signer;
#[cfg(target_arch = "wasm32")]
pub mod store;

/// Install the panic hook so a Rust panic shows up as a readable JS error.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

#[cfg(test)]
mod tests {
    use super::rules::AnyRules;
    use pubky_mayfly::rules::Rules;

    #[test]
    fn rules_are_found_by_id() {
        let r = AnyRules::by_id("list/1").expect("list/1 ships");
        assert_eq!(r.id(), "list/1");
        assert!(AnyRules::by_id("chess/1").is_none());
        assert_eq!(AnyRules::IDS, &["list/1"]);
    }
}
