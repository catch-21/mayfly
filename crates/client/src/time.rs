//! The one clock-shaped thing the client needs from its runtime: an async sleep.
//!
//! Natively that is `tokio::time::sleep`; in the browser it is a `setTimeout`. Nothing here
//! reads a wall clock — `ts` values come from the client's `clock` (§6.1), which the app sets.

use std::time::Duration;

/// Sleep for `d`.
#[cfg(not(target_arch = "wasm32"))]
pub async fn sleep(d: Duration) {
    tokio::time::sleep(d).await;
}

/// Sleep for `d`.
#[cfg(target_arch = "wasm32")]
pub async fn sleep(d: Duration) {
    let ms = u32::try_from(d.as_millis()).unwrap_or(u32::MAX);
    gloo_timers::future::TimeoutFuture::new(ms).await;
}
