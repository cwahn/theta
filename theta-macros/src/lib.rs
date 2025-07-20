// lib.rs - Root of macro crate
use proc_macro::TokenStream;

mod persistence;

/// Derives the PersistentActor trait for a struct.
///
/// # Attributes
///
/// - `#[snapshot(Type)]` - Specifies the snapshot type. If not provided,
///   defaults to `<Self as Actor>::Args`.
///
/// # Example
///
/// ```rust
/// #[derive(PersistentActor)]
/// #[snapshot(MySnapshot)]
/// struct MyActor {
///     // fields...
/// }
/// ```
#[proc_macro_derive(PersistentActor, attributes(snapshot))]
pub fn derive_persistent_actor(input: TokenStream) -> TokenStream {
    persistence::derive_persistent_actor_impl(input)
}
