#![allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]
//! TypeScript/WASM bridge for the Theta actor framework.
//!
//! Provides initialization functions and re-exports for using Theta actors
//! from JavaScript/TypeScript via wasm-bindgen.
use std::sync::OnceLock;

use theta::prelude::*;
/// Re-export from theta — these traits are defined in `theta::ts`.
pub use theta::ts::{TsActor, TsActorRef};
use wasm_bindgen::prelude::*;

static ROOT_CTX: OnceLock<RootContext> = OnceLock::new();

/// Prelude re-exports for use by generated WASM bindings.
pub mod prelude {
    pub use futures;
    pub use js_sys;
    pub use serde;
    pub use serde_wasm_bindgen;
    pub use theta::prelude::*;
    pub use theta_flume;
    pub use wasm_bindgen;
    pub use wasm_bindgen::prelude::*;
    pub use wasm_bindgen_futures;

    pub use crate::root_ctx;
}

/// Initialize the Theta actor system (local-only mode).
///
/// Must be called once before spawning any actors from JS.
#[wasm_bindgen(js_name = "initThetaLocal")]
pub fn init_theta_local() {
    let ctx = RootContext::init_local();

    ROOT_CTX
        .set(ctx)
        .expect("Theta runtime already initialized");
}

/// Initialize the Theta actor system with iroh networking (remote mode).
///
/// Creates an iroh endpoint and initializes the root context.
/// Returns the public key string for this peer.
#[cfg(feature = "remote")]
#[wasm_bindgen(js_name = "initThetaRemote")]
pub async fn init_theta_remote() -> Result<String, JsError> {
    let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .alpns(vec![b"theta".to_vec()])
        .bind()
        .await
        .map_err(|e| JsError::new(&format!("endpoint bind failed: {e}")))?;
    let ctx = RootContext::init(endpoint);
    let public_key = ctx.public_key().to_string();

    ROOT_CTX
        .set(ctx)
        .map_err(|_| JsError::new("Theta runtime already initialized"))?;

    Ok(public_key)
}

/// Get the public key of this peer. Only available after `initThetaRemote`.
#[cfg(feature = "remote")]
#[wasm_bindgen(js_name = "publicKey")]
pub fn public_key() -> Result<String, JsError> {
    let ctx = ROOT_CTX
        .get()
        .ok_or_else(|| JsError::new("Theta runtime not initialized"))?;

    Ok(ctx.public_key().to_string())
}

/// Get the global root context.
///
/// # Panics
/// Panics if `initTheta` / `initThetaRemote` has not been called.
pub fn root_ctx() -> &'static RootContext {
    ROOT_CTX
        .get()
        .expect("Theta runtime not initialized — call initTheta() or initThetaRemote() first")
}
