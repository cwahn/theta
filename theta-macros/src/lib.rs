// lib.rs - Root of macro crate
use proc_macro::TokenStream;

mod persistence;

#[proc_macro_derive(PersistentActor, attributes(snapshot))]
pub fn derive_persistent_actor(input: TokenStream) -> TokenStream {
    persistence::derive_persistent_actor_impl(input)
}
