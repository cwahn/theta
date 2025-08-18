// lib.rs - Root of macro crate
use proc_macro::TokenStream;

mod actor;

// todo Make Uuid optional for non-remote
#[proc_macro_attribute]
pub fn actor(args: TokenStream, input: TokenStream) -> TokenStream {
    actor::actor_impl(args, input)
}

#[proc_macro_derive(ActorArgs)]
pub fn derive_actor_args(input: TokenStream) -> TokenStream {
    actor::derive_actor_args_impl(input)
}
