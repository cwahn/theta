//! TypeScript/WASM integration traits.
//!
//! These traits are implemented by the `#[actor(ts)]` macro to enable
//! preserve-based ActorRef serialization at the JS boundary.
use wasm_bindgen::prelude::*;

use crate::{actor::Actor, prelude::ActorRef};

/// Trait implemented by `#[actor(ts)]` on the actor struct.
///
/// Associates the actor with its generated `XxxRef` wasm-bindgen wrapper
/// so that cross-crate `ActorRef<X>` return types can be wrapped without
/// requiring an explicit import of `XxxRef` at the call site.
pub trait TsActor: Actor {
    /// The generated `#[wasm_bindgen] struct XxxRef` for this actor.
    type WasmRef: TsActorRef<Self>;
}

/// Trait implemented on the generated `XxxRef` struct.
///
/// Provides construction and extraction of the inner `ActorRef`, plus
/// `Into<JsValue>` bound needed for preserve-based serde.
pub trait TsActorRef<A: TsActor>: Sized + Into<JsValue> {
    fn from_ref(actor_ref: ActorRef<A>) -> Self;

    fn inner_ref(&self) -> ActorRef<A>;

    fn from_js_value(val: JsValue) -> Result<Self, JsValue>;
}

/// Serialize a value to JsValue for the WASM↔JS boundary.
///
/// On WASM, cooperative tasks may leak the PEER task-local from remote
/// monitor readers. This helper temporarily clears PEER so that
/// `ActorRef::Serialize` takes the preserve branch (live JS class)
/// instead of the DTO branch (plain object).
#[cfg(feature = "remote")]
pub fn to_js_value<T: serde::Serialize>(val: &T) -> Result<JsValue, JsError> {
    crate::remote::peer::PEER.clear_scope(|| {
        serde_wasm_bindgen::to_value(val)
            .map_err(|e| JsError::new(&format!("Serialize failed: {e}")))
    })
}

#[cfg(not(feature = "remote"))]
pub fn to_js_value<T: serde::Serialize>(val: &T) -> Result<JsValue, JsError> {
    serde_wasm_bindgen::to_value(val).map_err(|e| JsError::new(&format!("Serialize failed: {e}")))
}

/// Deserialize a value from JsValue for the WASM↔JS boundary.
///
/// Mirrors `to_js_value`: clears PEER so that `ActorRef::Deserialize` takes
/// the preserve branch (extracting the live JS class) instead of routing
/// through `ActorRefDto` (which expects wire-format objects, not class instances).
#[cfg(feature = "remote")]
pub fn from_js_value<T: for<'de> serde::Deserialize<'de>>(val: JsValue) -> Result<T, JsError> {
    crate::remote::peer::PEER.clear_scope(|| {
        serde_wasm_bindgen::from_value(val)
            .map_err(|e| JsError::new(&format!("Deserialize failed: {e}")))
    })
}

#[cfg(not(feature = "remote"))]
pub fn from_js_value<T: for<'de> serde::Deserialize<'de>>(val: JsValue) -> Result<T, JsError> {
    serde_wasm_bindgen::from_value(val)
        .map_err(|e| JsError::new(&format!("Deserialize failed: {e}")))
}
