//! Procedural macros for the Theta actor framework.
//!
//! This crate provides the `#[actor]` attribute macro and `ActorArgs` derive macro
//! that simplify actor implementation by generating the necessary boilerplate code.

// lib.rs - Root of macro crate
use proc_macro::TokenStream;

mod actor;

/// Attribute macro for implementing actors with automatic message handling.
///
/// This macro generates the necessary boilerplate for actor implementation,
/// including message enum generation, message processing, and remote communication
/// support when enabled.
///
/// # Usage
///
/// ```no_run
/// use theta::prelude::*;
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Debug, Clone, ActorArgs)]
/// struct MyActor { value: i32 }
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// struct Increment(i32);
///
/// #[actor("12345678-1234-5678-9abc-123456789abc")]
/// impl Actor for MyActor {
///     type StateReport = Nil;
///
///     const _: () = {
///         async |Increment(amount): Increment| {
///             self.value += amount;
///         };
///     };
/// }
/// ```
///
/// # Arguments
///
/// The macro requires a UUID string that uniquely identifies this actor type
/// for remote communication. This UUID should be generated once and remain
/// constant for the lifetime of the actor type.
// todo Make Uuid optional for non-remote
#[proc_macro_attribute]
pub fn actor(args: TokenStream, input: TokenStream) -> TokenStream {
    actor::actor_impl(args, input)
}

/// Derive macro for actor initialization arguments.
///
/// This macro implements the `ActorArgs` trait for structs, enabling them
/// to be used as initialization parameters for actors. It also automatically
/// implements `From<&Self> for Self` using `Clone`, which is required for
/// actor persistence.
///
/// # Requirements
///
/// The struct must implement `Clone` for the automatic `From` implementation.
///
/// # Usage
///
/// ```no_run
/// use theta::prelude::*;
///
/// #[derive(Debug, Clone, ActorArgs)]
/// struct MyActorArgs {
///     name: String,
///     initial_value: i32,
/// }
/// ```
#[proc_macro_derive(ActorArgs)]
pub fn derive_actor_args(input: TokenStream) -> TokenStream {
    actor::derive_actor_args_impl(input)
}
