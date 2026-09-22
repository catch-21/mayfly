//! `Send`/`Sync` bounds that hold natively and vanish on wasm.
//!
//! Natively a [`Store`](crate::Store) or [`Signer`](crate::Signer) may be shared across tokio
//! worker threads, so the traits require `Send + Sync` and `async_trait` makes their futures
//! `Send`. In the browser there is one thread, and a store backed by a JavaScript object
//! (`JsValue`) is neither `Send` nor `Sync`; demanding them would make the JS-provided store
//! of §16.2.1 impossible to write. These aliases let one trait definition serve both.

/// `Send` natively; nothing on wasm.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + ?Sized> MaybeSend for T {}

/// `Send` natively; nothing on wasm.
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSend for T {}

/// `Sync` natively; nothing on wasm.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSync: Sync {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Sync + ?Sized> MaybeSync for T {}

/// `Sync` natively; nothing on wasm.
#[cfg(target_arch = "wasm32")]
pub trait MaybeSync {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSync for T {}

/// A boxed clock closure with the bounds the target allows.
#[cfg(not(target_arch = "wasm32"))]
pub type Clock = Box<dyn Fn() -> u64 + Send + Sync>;
/// A boxed clock closure with the bounds the target allows.
#[cfg(target_arch = "wasm32")]
pub type Clock = Box<dyn Fn() -> u64>;

// Async traits take the matching pair at every definition and impl:
//
//     #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
//     #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
//
// written out rather than hidden in a macro, so the choice is greppable where it applies.
