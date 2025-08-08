use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse_macro_input;

pub(crate) fn intention_impl(input: TokenStream) -> TokenStream {
    let body: TokenStream2 = input.into();

    let expanded = quote! {
        async fn process_msg(&mut self, ctx: Context<Self>) {
            #body
        }
    };
    TokenStream::from(expanded)
}

pub(crate) fn derive_actor_impl(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);

    match generate_actor_impl(&input) {
        Ok(tokens) => TokenStream::from(tokens),
        Err(err) => TokenStream::from(err.to_compile_error()),
    }
}

pub(crate) fn derive_actor_config_impl(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);

    match generate_actor_config_impl(&input) {
        Ok(tokens) => TokenStream::from(tokens),
        Err(err) => TokenStream::from(err.to_compile_error()),
    }
}

fn generate_actor_impl(input: &syn::DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;

    let expanded = quote! {
        impl ::theta::actor::Actor for #name {}
    };

    Ok(expanded)
}

fn generate_actor_config_impl(input: &syn::DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;

    let expanded = quote! {
        impl ::theta::actor::ActorConfig for #name {
            type Actor = Self;

            async fn initialize(
                ctx: ::theta::context::Context<Self::Actor>,
                cfg: &Self,
            ) -> Self::Actor {
                cfg.clone()
            }
        }
    };

    Ok(expanded)
}
