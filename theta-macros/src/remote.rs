use heck::{ToShoutySnakeCase, ToSnakeCase};
use proc_macro::TokenStream;
use quote::quote;
use syn::parse_macro_input;

pub(crate) fn serde_trait_impl(_args: TokenStream, input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::ItemTrait);

    match generate_serde_trait_impl(&input) {
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

fn generate_serde_trait_impl(input: &syn::ItemTrait) -> syn::Result<proc_macro2::TokenStream> {
    let trait_name = &input.ident;

    let impl_id_trait_definition_tokens = impl_id_trait_definition_tokens(input)?;

    let registry_ident = deserialize_fn_registry_ident(&trait_name);
    let dist_slice_ident = distributed_slice_ident(&trait_name);

    let deistributed_slice_tokens = distributed_slice_tokens(trait_name, &dist_slice_ident)?;
    let registry_tokens =
        deserialize_fn_registry_tokens(&trait_name, &registry_ident, &dist_slice_ident)?;

    let serialize_tokens = serde_object_serialize_tokens(&trait_name)?;
    let deserialize_tokens = serde_object_deserialize_tokens(&trait_name, &registry_ident)?;

    Ok(quote! {
        #input

        #impl_id_trait_definition_tokens

        #deistributed_slice_tokens

        #registry_tokens

        #serialize_tokens

        #deserialize_tokens
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

    let trait_name = &trait_path.segments.last().unwrap().ident;

    let impl_id_trait_name = impl_id_trait_ident(trait_name);
    let type_ = &input.self_ty;
    let uuid_str = args.value();
    let dist_slice_ident = distributed_slice_ident(trait_name);
    let register_fn_ident = register_fn_ident(trait_name, &uuid_str);

    Ok(quote! {
        #input

        impl #impl_id_trait_name for #type_ {
            fn impl_id(&self) -> ::theta::remote::serde::ImplId {
                ::uuid::uuid!(#uuid_str)
            }
        }

        #[::linkme::distributed_slice(#dist_slice_ident)]
        fn #register_fn_ident(registry: &mut DeserializeFnRegistry<dyn #trait_name>) {
            registry.register(::uuid::uuid!(#uuid_str),|d| {Ok(Box::new(::erased_serde::deserialize::<#type_>(d)?))} )
        }

    })
}

// Helper functions

fn impl_id_trait_definition_tokens(
    input: &syn::ItemTrait,
) -> syn::Result<proc_macro2::TokenStream> {
    let trait_name = &input.ident;
    let impl_id_trait_name = impl_id_trait_ident(&trait_name);

    Ok(quote! {
        pub(crate) trait #impl_id_trait_name: {
            fn impl_id(&self) -> ::theta::remote::serde::ImplId;
        }
    })
}

fn impl_id_trait_ident(trait_name: &syn::Ident) -> syn::Ident {
    syn::Ident::new(&format!("{}ImplId", trait_name), trait_name.span())
}

fn distributed_slice_tokens(
    trait_name: &syn::Ident,
    dist_slice_ident: &syn::Ident,
) -> syn::Result<proc_macro2::TokenStream> {
    Ok(quote! {
        #[::linkme::distributed_slice]
        static #dist_slice_ident: [fn(&mut DeserializeFnRegistry<dyn #trait_name>)] = [..];
    })
}

fn deserialize_fn_registry_tokens(
    serde_trait_name: &syn::Ident,
    registry_ident: &syn::Ident,
    dist_slice_ident: &syn::Ident,
) -> syn::Result<proc_macro2::TokenStream> {
    Ok(quote! {
        static #registry_ident: ::std::sync::LazyLock<
            ::theta::remote::serde::DeserializeFnRegistry<dyn #serde_trait_name>
        > = ::std::sync::LazyLock::new(|| {
            let mut registry = ::theta::remote::serde::DeserializeFnRegistry::new();

            for regiester_fn in #dist_slice_ident.iter() {
                regiester_fn(&mut registry);
            }

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

fn distributed_slice_ident(trait_name: &syn::Ident) -> syn::Ident {
    syn::Ident::new(
        &format!(
            "{}_DESERIALIZE_FN_REGISTER_FNS",
            trait_name.to_string().to_shouty_snake_case()
        ),
        trait_name.span(),
    )
}

fn register_fn_ident(trait_name: &syn::Ident, uuid_str: &String) -> syn::Ident {
    syn::Ident::new(
        &format!(
            "register_{}_{}",
            trait_name.to_string().to_snake_case(),
            uuid_str.replace("-", "_")
        ),
        trait_name.span(),
    )
}
