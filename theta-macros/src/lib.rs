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
/// # Default Implementations
///
/// The macro automatically provides default implementations for:
/// - `type StateReport = Nil;` - No state reporting by default
/// - `const _: () = {};` - Empty message handler block
///
/// You only need to specify these if you want custom behavior.
///
/// # Usage
///
/// ## Basic actor (no custom StateReport or message handlers)
/// ```ignore
/// use theta::prelude::*;
///
/// #[derive(Debug, Clone, ActorArgs)]
/// struct MyActor { value: i32 }
///
/// #[actor("12345678-1234-5678-9abc-123456789abc")]
/// impl Actor for MyActor {}
/// ```
///
/// ## Actor with message handlers
/// ```ignore
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
///
/// ## Optional Parameters
/// - `snapshot` - Enables persistence support, automatically implements `PersistentActor`
// todo Make Uuid optional for non-remote
#[proc_macro_attribute]
pub fn actor(args: TokenStream, input: TokenStream) -> TokenStream {
    actor::actor_impl(args, input)
}

/// Derive macro for actor argument types with automatic trait implementations.
///
/// This macro implements necessary traits for actor initialization arguments,
/// including `Clone` and automatic `From<&Self>` conversion for convenient
/// actor spawning patterns.
///
/// # Usage
///
/// ## Auto-args pattern (recommended)
/// When using the auto-args pattern with `ctx.spawn_auto`, the macro enables
/// convenient spawning without explicit conversion:
/// ```ignore
/// use theta::prelude::*;
///
/// #[derive(Debug, Clone, ActorArgs)]
/// struct MyActor { value: i32 }
///
/// #[actor("12345678-1234-5678-9abc-123456789abc")]
/// impl Actor for MyActor {}
///
/// // Usage: auto-args pattern
/// let actor_ref = ctx.spawn_auto(&MyActor { value: 42 }).await?;
/// ```
///
/// ## Custom implementation pattern
/// For actors with custom initialization logic:
/// ```ignore
/// use theta::prelude::*;
///
/// #[derive(Debug, Clone, ActorArgs)]
/// struct DatabaseActor {
///     connection_string: String,
///     pool_size: usize,
/// }
///
/// impl DatabaseActor {
///     pub fn new(connection_string: String) -> Self {
///         Self {
///             connection_string,
///             pool_size: 10, // default pool size
///         }
///     }
/// }
/// ```
///
/// # Generated Implementations
///
/// The macro automatically generates:
/// - `Clone` trait (required for all actor args)
/// - `From<&Self> for Self` - Enables reference-to-owned conversion for spawning
#[proc_macro_derive(ActorArgs)]
pub fn derive_actor_args(input: TokenStream) -> TokenStream {
    actor::derive_actor_args_impl(input)
}
