// lib.rs - Root of macro crate
use proc_macro::TokenStream;

mod actor;
mod persistence;

#[cfg(feature = "remote")]
mod remote;

#[proc_macro_derive(Actor)]
pub fn derive_actor(input: TokenStream) -> TokenStream {
    actor::derive_actor_impl(input)
}

#[proc_macro_derive(ActorConfig)]
pub fn derive_actor_config(input: TokenStream) -> TokenStream {
    actor::derive_actor_config_impl(input)
}

#[proc_macro_derive(PersistentActor, attributes(snapshot))]
pub fn derive_persistent_actor(input: TokenStream) -> TokenStream {
    persistence::derive_persistent_actor_impl(input)
}

#[cfg(feature = "remote")]
#[proc_macro_attribute]
pub fn type_id(args: TokenStream, input: TokenStream) -> TokenStream {
    remote::type_id_attr_impl(args, input)
}
