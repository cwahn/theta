// persistent_actor.rs - Implementation module
use heck::ToShoutySnakeCase;
use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, parse_macro_input};

/// Internal implementation of the PersistentActor derive macro.
pub(crate) fn derive_persistent_actor_impl(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    match generate_persistent_actor_impl(&input) {
        Ok(tokens) => TokenStream::from(tokens),
        Err(err) => TokenStream::from(err.to_compile_error()),
    }
}

/// Generates the implementation tokens for the PersistentActor trait.
fn generate_persistent_actor_impl(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let snapshot_type = find_snapshot_type(input)?;
    let registry_ident = create_registry_ident(name);

    let expanded = quote! {
        static #registry_ident: ::std::sync::LazyLock<::std::sync::RwLock<::theta::persistence::Bijection<::url::Url, ::theta::prelude::WeakActorRef<#name>>>> =
            ::std::sync::LazyLock::new(|| ::std::sync::RwLock::new(::theta::persistence::Bijection::new()));

        impl ::theta::persistence::PersistentActor for #name {
            type Snapshot = #snapshot_type;

            fn bind_persistent(
                persistence_key: ::url::Url,
                actor_ref: &::theta::prelude::ActorRef<Self>
            ) -> ::anyhow::Result<()> {
                let Ok(mut registry) = #registry_ident.write() else {
                    ::anyhow::bail!("Failed to acquire write lock on registry");
                };

                if let Some(old_pair) = registry.insert(persistence_key, actor_ref.downgrade()) {
                    #[cfg(feature = "tracing")]
                    ::tracing::warn!("Existing persistent actor reference for {old_pair:?} is replaced");
                }

                Ok(())
            }

            fn persistence_key(actor_ref: &::theta::prelude::ActorRef<Self>) -> Option<::url::Url> {
                let registry = #registry_ident.read().unwrap();
                registry.get_left(&actor_ref.downgrade()).cloned()
            }

            fn lookup_persistent(persistence_key: &::url::Url) -> Option<::theta::prelude::ActorRef<Self>> {
                let registry = #registry_ident.read().unwrap();
                registry
                    .get_right(persistence_key)
                    .and_then(|weak_ref| weak_ref.upgrade())
            }
        }
    };

    Ok(expanded)
}

/// Creates a unique registry identifier for the given struct name.
fn create_registry_ident(name: &syn::Ident) -> syn::Ident {
    syn::Ident::new(
        &format!("{}_REGISTRY", name.to_string().to_shouty_snake_case()),
        name.span(),
    )
}

/// Finds the snapshot type from the `#[snapshot(Type)]` attribute.
/// If not found, defaults to `<Self as Actor>::Args`.
fn find_snapshot_type(input: &DeriveInput) -> syn::Result<syn::Type> {
    // Look for #[snapshot(Type)] attribute
    for attr in &input.attrs {
        if attr.path().is_ident("snapshot") {
            return attr.parse_args::<syn::Type>();
        }
    }

    // Default snapshot type
    Ok(syn::parse_quote! { <Self as ::theta::prelude::Actor>::Args })
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;
    use syn::parse_quote;

    #[test]
    fn test_create_registry_ident() {
        let name: syn::Ident = parse_quote!(MyActor);
        let registry = create_registry_ident(&name);
        assert_eq!(registry.to_string(), "MY_ACTOR_REGISTRY");
    }

    #[test]
    fn test_find_snapshot_type_with_attribute() {
        let input: DeriveInput = parse_quote! {
            #[snapshot(MySnapshot)]
            struct MyActor;
        };

        let snapshot_type = find_snapshot_type(&input).unwrap();
        let expected: syn::Type = parse_quote!(MySnapshot);

        assert_eq!(
            quote!(#snapshot_type).to_string(),
            quote!(#expected).to_string()
        );
    }

    #[test]
    fn test_find_snapshot_type_default() {
        let input: DeriveInput = parse_quote! {
            struct MyActor;
        };

        let snapshot_type = find_snapshot_type(&input).unwrap();
        let expected: syn::Type = parse_quote!(<Self as ::theta::prelude::Actor>::Args);

        assert_eq!(
            quote!(#snapshot_type).to_string(),
            quote!(#expected).to_string()
        );
    }

    #[test]
    fn test_generate_persistent_actor_impl() {
        let input: DeriveInput = parse_quote! {
            #[snapshot(MySnapshot)]
            struct TestActor {
                field: i32,
            }
        };

        let result = generate_persistent_actor_impl(&input);
        assert!(result.is_ok());

        let tokens = result.unwrap();
        let token_string = tokens.to_string();

        // Verify key components are present (accounting for spacing in token stream)
        assert!(token_string.contains("TEST_ACTOR_REGISTRY"));
        assert!(
            token_string.contains("impl :: theta :: persistence :: PersistentActor for TestActor")
        );
        assert!(token_string.contains("type Snapshot = MySnapshot"));
    }
}
