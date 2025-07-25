use heck::ToShoutySnakeCase;
use proc_macro::TokenStream;
use quote::quote;
use syn::parse_macro_input;

pub(crate) fn serde_trait_impl(args: TokenStream, input: TokenStream) -> TokenStream {
    let serde_trait_name = parse_macro_input!(args as syn::Ident);
    let input = parse_macro_input!(input as syn::ItemTrait);

    match generate_serde_trait_impl(serde_trait_name, &input) {
        Ok(tokens) => TokenStream::from(tokens),
        Err(err) => TokenStream::from(err.to_compile_error()),
    }
}

pub(crate) fn impl_id_attr_impl(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as syn::LitStr);
    let input = parse_macro_input!(input as syn::ItemImpl);

    match generate_impl_id_attr_impl(&input, &args) {
        Ok(tokens) => TokenStream::from(tokens),
        Err(err) => TokenStream::from(err.to_compile_error()),
    }
}

// Implementations

fn generate_serde_trait_impl(
    serde_trait_name: syn::Ident,
    input: &syn::ItemTrait,
) -> syn::Result<proc_macro2::TokenStream> {
    let serde_trait_definition_tokens =
        serde_trait_definition_tokens(&serde_trait_name, &input.ident)?;

    let impl_id_trait_definition_tokens = impl_id_trait_definition_tokens(input)?;

    let registry_ident = deserialize_fn_registry_ident(&serde_trait_name);

    let registry_tokens = serde_object_registry_tokens(&serde_trait_name, &registry_ident)?;
    let serialize_tokens = serde_object_serialize_tokens(&serde_trait_name)?;
    let deserialize_tokens = serde_object_deserialize_tokens(&serde_trait_name, &registry_ident)?;

    Ok(quote! {
        #input

        #serde_trait_definition_tokens

        #impl_id_trait_definition_tokens

        // #registry_tokens

        // #serialize_tokens

        // #deserialize_tokens
    })
}

fn generate_impl_id_attr_impl(
    input: &syn::ItemImpl,
    args: &syn::LitStr,
) -> syn::Result<proc_macro2::TokenStream> {
    let trait_path = &input
        .trait_
        .as_ref()
        .ok_or_else(|| {
            syn::Error::new_spanned(input, "impl_id can only be used on trait implementations")
        })?
        .1;

    let impl_id_trait_name = impl_id_trait_ident(&trait_path.segments.last().unwrap().ident);
    let type_path = &input.self_ty;
    let uuid_str = args.value();

    Ok(quote! {
        #input

        impl #impl_id_trait_name for #type_path {
            fn impl_id(&self) -> ::theta::remote::serde::ImplId {
                ::uuid::uuid!(#uuid_str)
            }
        }
    })
}

// Helper functions

fn serde_trait_definition_tokens(
    serde_trait_name: &syn::Ident,
    trait_name: &syn::Ident,
) -> syn::Result<proc_macro2::TokenStream> {
    let impl_id_trait_name = impl_id_trait_ident(trait_name);

    Ok(quote! {
        pub trait #serde_trait_name:  erased_serde::Serialize + #impl_id_trait_name {}
        impl<T: #impl_id_trait_name + erased_serde::Serialize> #serde_trait_name for T {}
    })
}

fn impl_id_trait_definition_tokens(
    input: &syn::ItemTrait,
) -> syn::Result<proc_macro2::TokenStream> {
    let trait_name = &input.ident;
    let impl_id_trait_name = impl_id_trait_ident(&trait_name);

    Ok(quote! {
        pub(crate) trait #impl_id_trait_name: #trait_name {
            fn impl_id(&self) -> ::theta::remote::serde::ImplId;
        }
    })
}

fn impl_id_trait_ident(trait_name: &syn::Ident) -> syn::Ident {
    syn::Ident::new(&format!("{}ImplId", trait_name), trait_name.span())
}

// fn impl_id_trait_path(trait_path: &syn::Path) -> syn::Result<syn::Path> {
//     let mut new_path = trait_path.clone();

//     // Get the last segment
//     let last_segment = new_path
//         .segments
//         .last_mut()
//         .ok_or_else(|| syn::Error::new_spanned(trait_path, "Trait path cannot be empty"))?;

//     // Convert the identifier using the existing function
//     let new_ident = impl_id_trait_ident(&last_segment.ident);

//     // Replace the identifier in the last segment
//     last_segment.ident = new_ident;

//     Ok(new_path)
// }

fn serde_object_registry_tokens(
    serde_trait_name: &syn::Ident,
    registry_ident: &syn::Ident,
) -> syn::Result<proc_macro2::TokenStream> {
    Ok(quote! {
        static #registry_ident: ::std::sync::LazyLock<
            ::theta::remote::serde::DeserializeFnRegistry<dyn #serde_trait_name>
        > = ::std::sync::LazyLock::new(|| {
            let mut registry = ::theta::remote::serde::DeserializeFnRegistry::new();

            // Register deserialization functions here

            registry
        });
    })
}

fn serde_object_serialize_tokens(
    serde_trait_name: &syn::Ident,
) -> syn::Result<proc_macro2::TokenStream> {
    Ok(quote! {
        impl ::serde::Serialize for dyn #serde_trait_name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: ::serde::Serializer,
            {
                const fn __check_erased_serialize_supertrait<T: ?Sized + #serde_trait_name>() {
                    ::serde_flexitos::ser::require_erased_serialize_impl::<T>();
                }

                ::serde_flexitos::serialize_trait_object(
                    serializer,
                    <dyn #serde_trait_name>::impl_id(self),
                    self,
                )
            }
        }
    })
}

fn serde_object_deserialize_tokens(
    serde_trait_name: &syn::Ident,
    registry_ident: &syn::Ident,
) -> syn::Result<proc_macro2::TokenStream> {
    Ok(quote! {
        impl<'de> ::serde::Deserialize<'de> for Box<dyn #serde_trait_name>  {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: ::serde::Deserializer<'de>,
            {
                #registry_ident.deserialize_trait_object(deserializer)
            }
        }
    })
}

fn deserialize_fn_registry_ident(trait_name: &syn::Ident) -> syn::Ident {
    syn::Ident::new(
        &format!(
            "{}_DESERIALIZE_FN_REGISTRY",
            trait_name.to_string().to_shouty_snake_case()
        ),
        trait_name.span(),
    )
}
