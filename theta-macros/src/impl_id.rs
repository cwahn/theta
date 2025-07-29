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
    let register_behavior_fn_type = register_behavior_fn_type(&input.self_ty);

    let submit_register_actor_fn_tokens =
        submit_register_actor_fn_tokens(&input, &register_behavior_fn_type)?;
    let collect_register_behavior_tokens =
        collect_register_behavior_tokens(&input, &register_behavior_fn_type)?;
    // let impl_serialize_tokens = impl_serialize_tokens(&input)?;
    // let impl_deserialize_tokens = impl_deserialize_tokens(&input)?;

    Ok(quote! {
        #input

        #submit_register_actor_fn_tokens

        #collect_register_behavior_tokens

        // #impl_serialize_tokens

        // #impl_deserialize_tokens
    })
}

fn generate_behavior_impl_id_attr_impl(
    input: syn::ItemImpl,
    args: &syn::LitStr,
) -> syn::Result<proc_macro2::TokenStream> {
    let input = impl_id_const_inserted(input, args)?;
    let register_behavior_fn_type = register_behavior_fn_type(&input.self_ty);

    let submit_register_behavior_tokens =
        submit_register_behavior_tokens(&input, &register_behavior_fn_type)?;

    Ok(quote! {
        #input

        #submit_register_behavior_tokens
    })
}

// Helper functions

fn submit_register_actor_fn_tokens(
    input: &syn::ItemImpl,
    register_behavior_fn_type: &syn::Type,
) -> syn::Result<proc_macro2::TokenStream> {
    let type_name = &input.self_ty;

    Ok(quote! {
        ::inventory::submit! {
            ::theta::remote::RegisterActorFn(|actor_registry| {
                let mut msg_registry = ::theta::remote::MsgRegistry::<#type_name>::default();

                // for entry in ::inventory::iter::<::theta::remote::RegisterBehaviorFn<#type_name>> {
                for entry in ::inventory::iter::<#register_behavior_fn_type> {
                    (entry.0)(&mut msg_registry);
                }

                actor_registry.insert(
                    <#type_name as ::theta::actor::Actor>::__IMPL_ID,
                    ::theta::remote::ActorEntry {
                        serialize_fn: |a| {
                            let actor = a.downcast_ref::<::theta::actor_ref::ActorRef<#type_name>>()
                                .ok_or_else(|| ::anyhow::anyhow!("Failed to downcast to {}", stringify!(#type_name)))?;

                            Ok(::postcard::to_stdvec(actor)?)
                        },
                        msg_registry: ::std::boxed::Box::new(msg_registry),
                    },
                );
            })
        }
    })
}

fn collect_register_behavior_tokens(
    input: &syn::ItemImpl,
    register_behavior_fn_type: &syn::Type,
) -> syn::Result<proc_macro2::TokenStream> {
    let type_name = &input.self_ty;
    // let register_behavior_fn_type = register_behavior_fn_type(type_name);

    Ok(quote! {
        pub struct #register_behavior_fn_type(pub fn(&mut ::theta::remote::MsgRegistry<#type_name>));

        // ::inventory::collect!(::theta::remote::RegisterBehaviorFn<#type_name>);
        inventory::collect!(#register_behavior_fn_type);
    })
}

fn submit_register_behavior_tokens(
    input: &syn::ItemImpl,
    register_behavior_fn_type: &syn::Type,
) -> syn::Result<proc_macro2::TokenStream> {
    let actor_type_name = &input.self_ty;

    // Extract the message type from Behavior<M>
    let msg_type = extract_behavior_message_type(input)?;

    // Rest of your implementation
    Ok(quote! {
        inventory::submit! {
            // ::theta::remote::RegisterBehaviorFn::<#actor_type_name>(|msg_registry| {
            #register_behavior_fn_type(|msg_registry| {
                msg_registry.0.insert(
                    <#actor_type_name as ::theta::message::Behavior<#msg_type>>::__IMPL_ID,
                    ::theta::remote::registry::MsgEntry::<#actor_type_name> {
                        deserialize_fn: |d| {
                            Ok(::std::boxed::Box::new(::erased_serde::deserialize::<#msg_type>(d)?))
                        },
                        serialize_return_fn: |a| {
                            let ret = a.downcast_ref::<<#actor_type_name as ::theta::message::Behavior<#msg_type>>::Return>()
                                .ok_or_else(|| ::anyhow::anyhow!("Failed to downcast to {}", stringify!(#msg_type)))?;
                            Ok(::postcard::to_stdvec(ret)?)
                        },
                    },
                );
            })
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
        impl<'de> ::serde::Deserialize<'de> for ::std::boxed::Box<dyn ::theta::message::Message<#type_name>> {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: ::serde::Deserializer<'de>,
            {

                let msg_registry = ::theta::remote::ACTOR_REGISTRY
                    .get(&<#type_name as ::theta::actor::Actor>::__IMPL_ID)
                    .and_then(|actor_entry| actor_entry.msg_registry.downcast_ref::<::theta::remote::MsgRegistry<#type_name>>())
                    .ok_or_else(|| {
                        ::serde::de::Error::custom(format!("Failed to get MsgRegistry for {}", stringify!(#type_name)))
                    })?;

                <::theta::remote::MsgRegistry<#type_name> as ::theta::remote::serde::Registry>::deserialize_trait_object(
                    msg_registry, deserializer
                )
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

fn register_behavior_fn_type(type_: &syn::Type) -> syn::Type {
    match type_ {
        syn::Type::Path(type_path) => {
            let mut segments = type_path.path.segments.clone();

            if let Some(last_segment) = segments.last_mut() {
                // Create new identifier: RegisterBehaviorFn + original type name
                let original_name = &last_segment.ident;
                let new_name = format!("RegisterBehaviorFn{}", original_name);
                last_segment.ident = syn::Ident::new(&new_name, last_segment.ident.span());
                last_segment.arguments = syn::PathArguments::None;
            }

            syn::Type::Path(syn::TypePath {
                qself: type_path.qself.clone(),
                path: syn::Path {
                    leading_colon: type_path.path.leading_colon,
                    segments,
                },
            })
        }
        _ => {
            panic!("Expected a path type, got: {:?}", type_);
        }
    }
}
