//! WASM bridge for the Theta actor framework.
//!
//! Provides initialization functions and re-exports for using Theta actors
//! from JavaScript/TypeScript via wasm-bindgen.

use std::sync::OnceLock;

use theta::prelude::*;
use wasm_bindgen::prelude::*;

static ROOT_CTX: OnceLock<RootContext> = OnceLock::new();

/// Initialize the Theta actor system (local-only mode).
///
/// Must be called once before spawning any actors from JS.
#[wasm_bindgen(js_name = "initTheta")]
pub fn init_theta() {
    let ctx = RootContext::init_local();
    ROOT_CTX
        .set(ctx)
        .expect("Theta runtime already initialized");
}

/// Get the global root context.
///
/// # Panics
/// Panics if `initTheta` has not been called.
pub fn root_ctx() -> &'static RootContext {
    ROOT_CTX
        .get()
        .expect("Theta runtime not initialized — call initTheta() first")
}

/// Prelude re-exports for use by generated WASM bindings.
pub mod prelude {
    pub use theta::prelude::*;
    pub use theta_flume;
    pub use wasm_bindgen;
    pub use wasm_bindgen::prelude::*;
    pub use wasm_bindgen_futures;

    pub use crate::root_ctx;

    // Re-export serde-wasm-bindgen for generated code
    pub use serde_wasm_bindgen;

    // Re-export js-sys for generated code
    pub use js_sys;

    // Re-export serde for generated code
    pub use serde;
}
