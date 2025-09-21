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
/// - `type View = Nil;` - No state updateing by default
/// - `const _: () = {};` - Empty message handler block
///
/// You only need to specify these if you want custom behavior.
///
/// # Usage
///
/// ## Basic actor (no custom View or message handlers)
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
/// * `uuid` - A UUID string literal that uniquely identifies this actor type for remote
///   communication. Must be a valid UUID format (e.g., "12345678-1234-5678-9abc-123456789abc").
///   This UUID should be generated once and remain constant for the lifetime of the actor type.
///
/// ## Optional Parameters
/// * `snapshot` - Enables persistence support. When specified, automatically implements
///   `PersistentActor` trait for the actor type. Can be used as `snapshot` (defaults to `Self`)
///   or `snapshot = CustomType` for custom snapshot types.
///
/// # Return
///
/// Generates a complete actor implementation including:
/// * `Actor` trait implementation with message processing
/// * Message enum type containing all handled message variants
/// * `ProcessMessage` trait implementation for message routing
/// * Remote communication support with serialization/deserialization
/// * Optional `PersistentActor` implementation when `snapshot` parameter is used
///
/// # Errors
///
/// Compilation errors may occur if:
/// * UUID string is not a valid UUID format
/// * Actor implementation is malformed or missing required elements
/// * Message handler closures have invalid signatures
/// * Snapshot type (when specified) does not implement required traits
///
/// # Notes
///
/// * All message types used in handlers must implement `Serialize + Deserialize`
/// * Actor state type must implement `Clone + Debug`
/// * Message handlers are defined as async closures within `const _: () = {};` blocks
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
/// ``` ignore
/// use theta::prelude::*;
///
/// #[derive(Debug, Clone, ActorArgs)]
/// struct MyActor { value: i32 }
///
/// #[actor("12345678-1234-5678-9abc-123456789abc")]
/// impl Actor for MyActor {}
///
/// // Usage: auto-args pattern
/// let actor = ctx.spawn_auto(&MyActor { value: 42 }).await?;
/// ```
///
/// ## Custom implementation pattern
/// For actors with custom initialization logic:
/// ``` ignore
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
/// # Arguments
///
/// The derive macro is applied to struct types that serve as actor initialization arguments.
/// The struct must contain all fields necessary for actor initialization.
///
/// # Return
///
/// Automatically generates implementations for:
/// * `Clone` trait - Required for all actor argument types to enable multiple spawning
/// * `From<&Self> for Self` trait - Enables convenient reference-to-owned conversion
///   for the auto-args spawning pattern used with `ctx.spawn_auto()`
///
/// # Errors
///
/// Compilation errors may occur if:
/// * Applied to non-struct types (enums, unions not supported)
/// * Struct contains fields that do not implement `Clone`
/// * Struct has generic parameters that don't meet trait bounds
///
/// # Notes
///
/// * This derive macro is specifically designed for actor argument structs
/// * Works seamlessly with the `#[actor]` attribute macro
/// * Enables both direct instantiation and auto-args spawning patterns
/// * All fields in the struct must be cloneable for the generated `Clone` implementation
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
