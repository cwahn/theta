// Thin cdylib entry point — ensures macro-generated wasm_bindgen exports
// from the shared crate are included in the WASM binary.
// All app logic lives in JavaScript calling the generated API.

// Force the shared crate to be linked so wasm-bindgen descriptors survive.
pub use web_chat_shared as _shared;

// theta-wasm provides initThetaRemote / publicKey — re-export so they link too.
pub use theta_wasm as _wasm;
