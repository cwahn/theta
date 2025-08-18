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
/// ```no_run
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

/// Derive macro for actor initialization arguments.
///
/// This macro implements the `ActorArgs` trait for structs, enabling them
/// to be used as initialization parameters for actors. It provides automatic
/// implementation where the actor is initialized by cloning the arguments.
///
/// The macro also automatically implements `From<&Self> for Self` using `Clone`,
/// which is required for actor persistence.
///
/// # When to Use
///
/// - **Auto-args**: Use `#[derive(ActorArgs)]` on your actor struct when the actor
///   can be initialized directly from its own fields (most common case)
/// - **Custom initialization**: Manually implement `ActorArgs` when you need
///   separate argument types or custom initialization logic
///
/// # Requirements
///
/// The struct must implement `Clone` for the automatic implementations.
///
/// # Usage
///
/// ## Basic usage (actor as its own args)
/// ```no_run
/// use theta::prelude::*;
///
/// #[derive(Debug, Clone, ActorArgs)]
/// struct Counter {
///     initial_value: i32,
/// }
///
/// #[actor("12345678-1234-5678-9abc-123456789abc")]
/// impl Actor for Counter {}
/// ```
///
/// ## Manual implementation for custom args
/// ```no_run
/// use theta::prelude::*;
///
/// struct MyActor { 
///     value: i32,
///     name: String,
/// }
///
/// struct MyActorArgs {
///     initial_value: i32,
/// }
///
/// impl ActorArgs for MyActorArgs {
///     type Actor = MyActor;
///     
///     async fn initialize(ctx: Context<MyActor>, args: &Self) -> MyActor {
///         MyActor {
///             value: args.initial_value,
///             name: "default".to_string(),
///         }
///     }
/// }
/// ```
#[proc_macro_derive(ActorArgs)]
pub fn derive_actor_args(input: TokenStream) -> TokenStream {
    actor::derive_actor_args_impl(input)
}
