//! TypeScript/WASM integration traits.
//!
//! These traits are implemented by the `#[actor(ts)]` macro to enable
//! preserve-based ActorRef serialization at the JS boundary.

use crate::actor::Actor;
use crate::prelude::ActorRef;
use wasm_bindgen::prelude::*;

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
/// `Into<JsValue>` and `JsCast` bounds needed for preserve-based serde.
pub trait TsActorRef<A: TsActor>: Sized + Into<JsValue> + JsCast {
    fn from_ref(actor_ref: ActorRef<A>) -> Self;
    fn inner_ref(&self) -> ActorRef<A>;
}
