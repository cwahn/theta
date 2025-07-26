use proc_macro::TokenStream;
use quote::quote;
use syn::parse_macro_input;

pub(crate) fn impl_id_attr_impl(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as syn::LitStr);
    let input = parse_macro_input!(input as syn::ItemImpl);

    match generate_impl_id_attr_impl(input, args) {
        Ok(tokens) => TokenStream::from(tokens),
        Err(err) => TokenStream::from(err.to_compile_error()),
    }
}

// Implementations

fn generate_impl_id_attr_impl(
    input: syn::ItemImpl,
    args: syn::LitStr,
) -> syn::Result<proc_macro2::TokenStream> {
    let trait_path = &input
        .trait_
        .as_ref()
        .ok_or_else(|| {
            syn::Error::new_spanned(
                input.clone(),
                "impl_id can only be used on trait implementations",
            )
        })?
        .1;

    let trait_name = &trait_path.segments.last().unwrap().ident;

    match trait_name.to_string().as_str() {
        "Actor" => generate_actor_impl_id_attr_impl(input, &args),
        "Behavior" => generate_behavior_impl_id_attr_impl(input, &args),
        _ => Err(syn::Error::new_spanned(
            trait_name,
            "impl_id can only be used on Actor or Behavior trait implementations",
        )),
    }
}

fn generate_actor_impl_id_attr_impl(
    input: syn::ItemImpl,
    args: &syn::LitStr,
) -> syn::Result<proc_macro2::TokenStream> {
    let input = impl_id_const_inserted(input, args)?;

    let submit_actor_init_fn_tokens = submit_actor_init_fn_tokens(&input)?;
    let impl_serialize_tokens = impl_serialize_tokens(&input)?;
    let impl_deserialize_tokens = impl_deserialize_tokens(&input)?;

    Ok(quote! {
        #input

        #submit_actor_init_fn_tokens

        #impl_serialize_tokens

        #impl_deserialize_tokens
    })
}

fn generate_behavior_impl_id_attr_impl(
    input: syn::ItemImpl,
    args: &syn::LitStr,
) -> syn::Result<proc_macro2::TokenStream> {
    let input = impl_id_const_inserted(input, args)?;

    let submit_msg_deserialize_entry_tokens = submit_msg_deserialize_entry_tokens(&input)?;

    Ok(quote! {
        #input

        #submit_msg_deserialize_entry_tokens
    })
}

// Helper functions

fn submit_actor_init_fn_tokens(input: &syn::ItemImpl) -> syn::Result<proc_macro2::TokenStream> {
    let type_name = &input.self_ty;

    Ok(quote! {
        ::inventory::submit! {
            ::theta::remote::RegisterActorFn(||{
                let mut registry = ::std::boxed::Box::new(::theta::remote::serde::DeserializeFnRegistry::<dyn ::theta::message::Message<#type_name>>::new())
                    as ::std::boxed::Box<dyn ::std::any::Any + Send + Sync>;

                for entry in ::inventory::iter::<::theta::remote::MsgEntry> {
                    if entry.actor_impl_id == <#type_name as ::theta::actor::Actor>::__IMPL_ID {
                        (entry.register_fn)(&mut registry);
                    }
                }

                ::theta::remote::REGISTRY.write().unwrap().insert(
                    <#type_name as ::theta::actor::Actor>::__IMPL_ID,
                    registry,
                );
            })
        }
    })
}

fn submit_msg_deserialize_entry_tokens(
    input: &syn::ItemImpl,
) -> syn::Result<proc_macro2::TokenStream> {
    let actor_type_name = &input.self_ty;

    // Extract the message type from Behavior<M>
    let msg_type = extract_behavior_message_type(input)?;

    // Rest of your implementation
    Ok(quote! {
        inventory::submit! {
            ::theta::remote::MsgEntry {
                actor_impl_id: <#actor_type_name as ::theta::actor::Actor>::__IMPL_ID,
                register_fn: |registry| {
                    if let Some(reg) = registry.downcast_mut::<DeserializeFnRegistry<dyn Message<#actor_type_name>>>() {
                        reg.register(
                            <#actor_type_name as Behavior<#msg_type>>::__IMPL_ID,
                            |d| {
                                Ok(Box::new(
                                    erased_serde::deserialize::<#msg_type>(d)?,
                                ))
                            },
                        );
                    } else {
                        panic!("Failed to downcast registry to DeserializeFnRegistry<dyn Message<{}>>", stringify!(#actor_type_name));
                    }
                }
            }
        }
    })
}

fn extract_behavior_message_type(input: &syn::ItemImpl) -> syn::Result<&syn::Type> {
    let trait_path = &input
        .trait_
        .as_ref()
        .ok_or_else(|| syn::Error::new_spanned(input, "Expected trait implementation"))?
        .1;

    let behavior_segment = trait_path
        .segments
        .iter()
        .find(|segment| segment.ident == "Behavior")
        .ok_or_else(|| {
            syn::Error::new_spanned(trait_path, "Expected Behavior trait implementation")
        })?;

    let generic_args = match &behavior_segment.arguments {
        syn::PathArguments::AngleBracketed(args) => args,
        _ => {
            return Err(syn::Error::new_spanned(
                behavior_segment,
                "Expected generic arguments in Behavior<M>",
            ));
        }
    };

    let message_type_arg = generic_args.args.first().ok_or_else(|| {
        syn::Error::new_spanned(
            generic_args,
            "Expected message type argument in Behavior<M>",
        )
    })?;

    // Extract the type from the generic argument
    match message_type_arg {
        syn::GenericArgument::Type(ty) => Ok(ty),
        _ => Err(syn::Error::new_spanned(
            message_type_arg,
            "Expected type argument in Behavior<M>",
        )),
    }
}

fn impl_serialize_tokens(input: &syn::ItemImpl) -> syn::Result<proc_macro2::TokenStream> {
    let type_name = &input.self_ty;

    Ok(quote! {
        impl ::serde::Serialize for dyn ::theta::message::Message<#type_name> {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: ::serde::Serializer,
            {
                const fn __check_erased_serialize_supertrait<T: ?Sized + ::theta::message::Message<#type_name>>() {
                    ::serde_flexitos::ser::require_erased_serialize_impl::<T>();
                }

                ::serde_flexitos::serialize_trait_object(serializer, self.__impl_id(), self)
            }
        }
    })
}

fn impl_deserialize_tokens(input: &syn::ItemImpl) -> syn::Result<proc_macro2::TokenStream> {
    let type_name = &input.self_ty;

    Ok(quote! {
        impl<'de> ::serde::Deserialize<'de> for Box<dyn ::theta::message::Message<#type_name>> {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: ::serde::Deserializer<'de>,
            {
                let type_system = ::theta::remote::REGISTRY.read().unwrap();

                let registry = type_system
                    .get(&<#type_name as ::theta::actor::Actor>::__IMPL_ID)
                    .and_then(|v| v.downcast_ref::<::theta::remote::serde::DeserializeFnRegistry<dyn ::theta::message::Message<#type_name>>>())
                    .ok_or_else(|| {
                        ::serde::de::Error::custom(format!("Failed to get DeserializeFnRegistry for {}", stringify!(#type_name)))
                    })?;

                registry.deserialize_trait_object(deserializer)
            }
        }
    })
}

fn impl_id_const_inserted(
    mut input: syn::ItemImpl,
    impl_id: &syn::LitStr,
) -> syn::Result<syn::ItemImpl> {
    let impl_id_const = quote! {
        const __IMPL_ID: ::theta::base::ImplId = ::uuid::uuid!(#impl_id);
    };

    let const_item: syn::ImplItemConst = syn::parse2(impl_id_const)?;

    input.items.push(syn::ImplItem::Const(const_item));

    Ok(input)
}
