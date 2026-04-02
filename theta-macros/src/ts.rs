//! TypeScript/WASM binding generation for actors.
//!
//! When `ts` is specified on `#[actor]`, this module generates:
//! - `typescript_custom_section` with TS type definitions
//! - `{Actor}Ref` wasm-bindgen class (tell, ask, prep, initStream)
//! - Free functions: spawn{Actor}, lookup{Actor}, lookup{Actor}Local, bind{Actor}

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{Type, TypePath};

/// Information about a single message handler needed for TS generation.
pub(crate) struct TsMsgInfo {
    /// The Rust message type name (e.g. `SendMessage`)
    pub rust_type_name: String,
    /// The variant ident in the Msg enum (e.g. `__SendMessage`)
    pub variant_ident: syn::Ident,
    /// Whether this handler has a return type (ask pattern)
    pub return_type: Option<syn::Type>,
}

/// If the type is `ActorRef<X>`, return the inner actor ident `X`.
fn extract_actor_ref_inner(ty: &Type) -> Option<syn::Ident> {
    let Type::Path(type_path) = ty else {
        return None;
    };

    let seg = type_path.path.segments.last()?;

    if seg.ident != "ActorRef" {
        return None;
    }

    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };

    let Some(syn::GenericArgument::Type(Type::Path(inner_path))) = args.args.first() else {
        return None;
    };

    let inner_seg = inner_path.path.segments.last()?;

    Some(inner_seg.ident.clone())
}

/// Check if the type is `()` (unit type).
fn is_unit_type(ty: &Type) -> bool {
    if let Type::Tuple(tuple) = ty {
        return tuple.elems.is_empty();
    }
    false
}

/// Generate all TS/WASM bindings for an actor.
///
/// Returns a token stream to be appended to the macro output, gated behind
/// `#[cfg(all(feature = "ts", target_arch = "wasm32"))]`.
pub(crate) fn generate_ts_bindings(
    actor_ident: &syn::Ident,
    view_type: &TypePath,
    msg_infos: &[TsMsgInfo],
    enum_ident: &syn::Ident,
) -> syn::Result<TokenStream2> {
    let actor_name = actor_ident.to_string();
    let ref_ident = format_ident!("{}Ref", actor_ident);
    let ts_msg_enum_ident = format_ident!("{}__TsMsg", actor_ident);
    let mod_ident = format_ident!("__ts_{}", actor_ident);

    // --- 1. typescript_custom_section ---
    let ts_types = generate_ts_type_section(&actor_name, &ref_ident, view_type, msg_infos);

    // --- 2. TsMsg enum + From<JsValue> ---
    let ts_msg_enum = generate_ts_msg_enum(&ts_msg_enum_ident, enum_ident, msg_infos);

    // --- 3. Ref class ---
    let ref_class = generate_ref_class(
        actor_ident,
        &ref_ident,
        &ts_msg_enum_ident,
        enum_ident,
        view_type,
        msg_infos,
    );

    // --- 4. Free functions ---
    let free_fns = generate_free_functions(actor_ident, &ref_ident);

    // --- 5. TsActor impl (outside the mod to be in the crate root scope) ---
    let ts_actor_impl = generate_ts_actor_impl(actor_ident, &ref_ident);

    Ok(quote! {
        // All TS/WASM bindings are behind ts feature + wasm32 target
        #[cfg(all(feature = "ts", target_arch = "wasm32"))]
        #[allow(non_snake_case, unused_must_use)]
        mod #mod_ident {
            use super::*;
            use ::theta_ts::prelude::*;

            #ts_types
            #ts_msg_enum
            #ref_class
            #free_fns
        }

        // Re-export the Ref struct so other actors in the same crate can use it
        #[cfg(all(feature = "ts", target_arch = "wasm32"))]
        pub use #mod_ident::#ref_ident;

        // Implement TsActor for cross-crate ActorRef<X> return type support
        #[cfg(all(feature = "ts", target_arch = "wasm32"))]
        #ts_actor_impl
    })
}

/// Generate the `typescript_custom_section` with TS type definitions.
fn generate_ts_type_section(
    actor_name: &str,
    ref_ident: &syn::Ident,
    view_type: &TypePath,
    msg_infos: &[TsMsgInfo],
) -> TokenStream2 {
    // Resolve view type to TS
    let view_ts = crate::ts_type::rust_type_to_ts_from_path(view_type);
    // Build Msg discriminated union: { SendMessage: { text: string } } | ...
    // For now we use opaque types since we can't introspect Rust struct fields in proc macros.
    // Users will refine via #[message(ts)] later.
    let msg_variants: Vec<String> = msg_infos
        .iter()
        .map(|info| format!("{{ {0}: {0} }}", info.rust_type_name))
        .collect();
    let msg_union = if msg_variants.is_empty() {
        "never".to_string()
    } else {
        msg_variants.join(" | ")
    };

    // Build Returns map: { SendMessage: void, GetHistory: ChatHistory[] }
    let returns_entries: Vec<String> = msg_infos
        .iter()
        .map(|info| {
            let ret = match &info.return_type {
                Some(ty) => crate::ts_type::rust_type_to_ts_string(ty),
                None => "void".to_string(),
            };
            format!("  {}: {};", info.rust_type_name, ret)
        })
        .collect();
    let returns_body = returns_entries.join("\n");

    let ref_name = ref_ident.to_string();

    // Build ask overloads for declaration merging with the Ref class
    let ask_overloads: Vec<String> = msg_infos
        .iter()
        .map(|info| {
            let ret_ts = match &info.return_type {
                Some(ty) => crate::ts_type::rust_type_to_ts_string(ty),
                None => "void".to_string(),
            };
            format!(
                "  ask(msg: {{ {0}: {0} }}): Promise<{1}>;",
                info.rust_type_name, ret_ts
            )
        })
        .collect();

    let ref_interface = if msg_infos.is_empty() {
        String::new()
    } else {
        let mut lines = vec![format!("  tell(msg: {}Msg): void;", actor_name)];
        lines.extend(ask_overloads);
        lines.push(format!("  prep(): Promise<{}View>;", actor_name));
        lines.push(format!(
            "  initStream(callback: (state: {}View) => void): Promise<void>;",
            actor_name
        ));
        format!(
            "\nexport interface {} {{\n{}\n}}\n",
            ref_name,
            lines.join("\n")
        )
    };

    let ts_section = format!(
        r#"
export type {actor_name}Msg = {msg_union};

export interface {actor_name}Returns {{
{returns_body}
}}

export type {actor_name}View = {view_ts};

export interface {actor_name} {{
  readonly Msg: {actor_name}Msg;
  readonly Returns: {actor_name}Returns;
  readonly View: {actor_name}View;
}}
{ref_interface}"#,
        actor_name = actor_name,
        msg_union = msg_union,
        returns_body = returns_body,
        view_ts = view_ts,
        ref_interface = ref_interface,
    );

    quote! {
        #[wasm_bindgen(typescript_custom_section)]
        const _: &'static str = #ts_section;
    }
}

/// Generate `{Actor}__TsMsg` enum that mirrors the Rust Msg enum
/// but uses externally-tagged serde for JS interop.
fn generate_ts_msg_enum(
    ts_msg_enum_ident: &syn::Ident,
    rust_enum_ident: &syn::Ident,
    msg_infos: &[TsMsgInfo],
) -> TokenStream2 {
    if msg_infos.is_empty() {
        return quote! {};
    }

    // Generate variants: SendMessage(SendMessage), GetHistory(GetHistory), ...
    let variant_defs: Vec<TokenStream2> = msg_infos
        .iter()
        .map(|info| {
            let name = format_ident!("{}", info.rust_type_name);
            quote! { #name(#name) }
        })
        .collect();

    // Generate From<TsMsg> for RustMsg conversion arms
    let from_arms: Vec<TokenStream2> = msg_infos
        .iter()
        .map(|info| {
            let ts_variant = format_ident!("{}", info.rust_type_name);
            let rust_variant = &info.variant_ident;
            quote! {
                #ts_msg_enum_ident::#ts_variant(inner) => #rust_enum_ident::#rust_variant(inner)
            }
        })
        .collect();

    quote! {
        #[derive(::theta_ts::prelude::serde::Deserialize)]
        #[allow(non_camel_case_types)]
        enum #ts_msg_enum_ident {
            #(#variant_defs),*
        }

        impl From<#ts_msg_enum_ident> for #rust_enum_ident {
            fn from(ts_msg: #ts_msg_enum_ident) -> Self {
                match ts_msg {
                    #(#from_arms),*
                }
            }
        }
    }
}

/// Generate the `{Actor}Ref` wasm-bindgen class.
fn generate_ref_class(
    actor_ident: &syn::Ident,
    ref_ident: &syn::Ident,
    ts_msg_enum_ident: &syn::Ident,
    rust_enum_ident: &syn::Ident,
    _view_type: &TypePath,
    msg_infos: &[TsMsgInfo],
) -> TokenStream2 {
    let has_messages = !msg_infos.is_empty();

    let tell_method = if has_messages {
        quote! {
            #[wasm_bindgen(skip_typescript)]
            pub fn tell(&self, msg: wasm_bindgen::JsValue) -> Result<(), wasm_bindgen::JsError> {
                let ts_msg: #ts_msg_enum_ident = serde_wasm_bindgen::from_value(msg)
                    .map_err(|e| wasm_bindgen::JsError::new(&format!("Invalid message: {e}")))?;
                self.inner
                    .send(ts_msg.into(), ::theta::message::Continuation::Nil)
                    .map_err(|e| wasm_bindgen::JsError::new(&format!("Tell failed: {e}")))
            }
        }
    } else {
        quote! {}
    };

    // Generate per-variant ask dispatch arms
    let ask_arms: Vec<TokenStream2> = msg_infos
        .iter()
        .map(|info| {
            let ts_variant = format_ident!("{}", info.rust_type_name);
            let rust_variant = &info.variant_ident;

            let is_void = match &info.return_type {
                None => true,
                Some(ty) => is_unit_type(ty),
            };

            if is_void {
                // No return type or returns () — send with Reply continuation, await, return undefined
                quote! {
                    #ts_msg_enum_ident::#ts_variant(inner) => {
                        let rust_msg: <#actor_ident as ::theta::actor::Actor>::Msg =
                            #rust_enum_ident::#rust_variant(inner);
                        let (tx, rx) = futures::channel::oneshot::channel::<Box<dyn ::std::any::Any + Send>>();
                        self.inner
                            .send(rust_msg, ::theta::message::Continuation::Reply(tx))
                            .map_err(|e| wasm_bindgen::JsError::new(&format!("Ask failed: {e}")))?;
                        rx.await
                            .map_err(|_| wasm_bindgen::JsError::new("Ask cancelled — actor dropped"))?;
                        Ok(wasm_bindgen::JsValue::UNDEFINED)
                    }
                }
            } else if let Some(ty) = &info.return_type {
                if let Some(inner_actor) = extract_actor_ref_inner(ty) {
                    // Returns ActorRef<X> → wrap using TsActor trait (fully qualified,
                    // works across crates without requiring an explicit XxxRef import)
                    quote! {
                        #ts_msg_enum_ident::#ts_variant(inner) => {
                            let rust_msg: <#actor_ident as ::theta::actor::Actor>::Msg =
                                #rust_enum_ident::#rust_variant(inner);
                            let (tx, rx) = futures::channel::oneshot::channel::<Box<dyn ::std::any::Any + Send>>();
                            self.inner
                                .send(rust_msg, ::theta::message::Continuation::Reply(tx))
                                .map_err(|e| wasm_bindgen::JsError::new(&format!("Ask failed: {e}")))?;
                            let any_result = rx.await
                                .map_err(|_| wasm_bindgen::JsError::new("Ask cancelled — actor dropped"))?;
                            let result = *any_result
                                .downcast::<ActorRef<#inner_actor>>()
                                .map_err(|_| wasm_bindgen::JsError::new("Return type mismatch"))?;
                            Ok(<#inner_actor as ::theta_ts::TsActor>::WasmRef::from_ref(result).into())
                        }
                    }
                } else {
                    // Returns a serializable type → downcast and serialize
                    let return_type = ty;
                    quote! {
                        #ts_msg_enum_ident::#ts_variant(inner) => {
                            let rust_msg: <#actor_ident as ::theta::actor::Actor>::Msg =
                                #rust_enum_ident::#rust_variant(inner);
                            let (tx, rx) = futures::channel::oneshot::channel::<Box<dyn ::std::any::Any + Send>>();
                            self.inner
                                .send(rust_msg, ::theta::message::Continuation::Reply(tx))
                                .map_err(|e| wasm_bindgen::JsError::new(&format!("Ask failed: {e}")))?;
                            let any_result = rx.await
                                .map_err(|_| wasm_bindgen::JsError::new("Ask cancelled — actor dropped"))?;
                            let result = *any_result
                                .downcast::<#return_type>()
                                .map_err(|_| wasm_bindgen::JsError::new("Return type mismatch"))?;
                            serde_wasm_bindgen::to_value(&result)
                                .map_err(|e| wasm_bindgen::JsError::new(&format!("Serialize failed: {e}")))
                        }
                    }
                }
            } else {
                unreachable!("is_void=false implies return_type is Some")
            }
        })
        .collect();

    let ask_method = if !ask_arms.is_empty() {
        quote! {
            #[wasm_bindgen(skip_typescript)]
            pub async fn ask(&self, msg: wasm_bindgen::JsValue) -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsError> {
                let ts_msg: #ts_msg_enum_ident = serde_wasm_bindgen::from_value(msg)
                    .map_err(|e| wasm_bindgen::JsError::new(&format!("Invalid message: {e}")))?;
                match ts_msg {
                    #(#ask_arms)*
                }
            }
        }
    } else {
        quote! {}
    };

    quote! {
        #[wasm_bindgen]
        pub struct #ref_ident {
            inner: ActorRef<#actor_ident>,
        }

        #[wasm_bindgen]
        impl #ref_ident {
            #[wasm_bindgen(getter)]
            pub fn id(&self) -> String {
                self.inner.id().to_string()
            }

            #tell_method
            #ask_method

            #[wasm_bindgen]
            pub async fn prep(&self) -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsError> {
                let (tx, rx) = ::theta_ts::prelude::theta_flume::unbounded_anonymous();

                #[cfg(feature = "remote")]
                self.inner.monitor(tx).await
                    .map_err(|e| wasm_bindgen::JsError::new(&format!("Monitor failed: {e}")))?;

                #[cfg(not(feature = "remote"))]
                self.inner.monitor_local(tx)
                    .map_err(|e| wasm_bindgen::JsError::new(&format!("Monitor failed: {e}")))?;

                match rx.recv().await {
                    Some(::theta::monitor::Update::State(view)) => {
                        serde_wasm_bindgen::to_value(&view)
                            .map_err(|e| wasm_bindgen::JsError::new(&format!("Serialize failed: {e}")))
                    }
                    _ => Ok(wasm_bindgen::JsValue::NULL),
                }
            }

            #[wasm_bindgen(js_name = "initStream")]
            pub async fn init_stream(&self, callback: js_sys::Function) -> Result<(), wasm_bindgen::JsError> {
                let (tx, rx) = ::theta_ts::prelude::theta_flume::unbounded_anonymous();

                #[cfg(feature = "remote")]
                self.inner.monitor(tx).await
                    .map_err(|e| wasm_bindgen::JsError::new(&format!("Monitor failed: {e}")))?;

                #[cfg(not(feature = "remote"))]
                self.inner.monitor_local(tx)
                    .map_err(|e| wasm_bindgen::JsError::new(&format!("Monitor failed: {e}")))?;

                ::theta_ts::prelude::wasm_bindgen_futures::spawn_local(async move {
                    while let Some(update) = rx.recv().await {
                        match update {
                            ::theta::monitor::Update::State(view) => {
                                let Ok(js_val) = serde_wasm_bindgen::to_value(&view) else {
                                    continue;
                                };

                                let _ = callback.call1(
                                    &wasm_bindgen::JsValue::NULL,
                                    &js_val,
                                );
                            }
                            ::theta::monitor::Update::Status(_status) => {}
                        }
                    }
                });

                Ok(())
            }
        }

        impl ::theta_ts::TsActorRef<#actor_ident> for #ref_ident {
            fn from_ref(actor_ref: ActorRef<#actor_ident>) -> Self {
                Self { inner: actor_ref }
            }
        }

        impl #ref_ident {
            pub fn from_ref(actor_ref: ActorRef<#actor_ident>) -> Self {
                <Self as ::theta_ts::TsActorRef<#actor_ident>>::from_ref(actor_ref)
            }
        }
    }
}

fn generate_ts_actor_impl(actor_ident: &syn::Ident, ref_ident: &syn::Ident) -> TokenStream2 {
    quote! {
        impl ::theta_ts::TsActor for #actor_ident {
            type WasmRef = #ref_ident;
        }
    }
}

/// Generate free functions: spawn{Actor}, lookup{Actor}, lookup{Actor}Local, bind{Actor}.
fn generate_free_functions(actor_ident: &syn::Ident, ref_ident: &syn::Ident) -> TokenStream2 {
    let spawn_fn = format_ident!("spawn{}", actor_ident);
    let lookup_fn = format_ident!("lookup{}", actor_ident);
    let lookup_local_fn = format_ident!("lookup{}Local", actor_ident);
    let bind_fn = format_ident!("bind{}", actor_ident);

    quote! {
        #[wasm_bindgen(js_name = #spawn_fn)]
        pub fn #spawn_fn(args: wasm_bindgen::JsValue) -> Result<#ref_ident, wasm_bindgen::JsError> {
            let actor_args: #actor_ident = serde_wasm_bindgen::from_value(args)
                .map_err(|e| wasm_bindgen::JsError::new(&format!("Invalid args: {e}")))?;
            let actor_ref = root_ctx().spawn(actor_args);
            Ok(#ref_ident::from_ref(actor_ref))
        }

        #[wasm_bindgen(js_name = #lookup_local_fn)]
        pub fn #lookup_local_fn(name: &str) -> Result<#ref_ident, wasm_bindgen::JsError> {
            let actor_ref = ActorRef::<#actor_ident>::lookup_local(name)
                .map_err(|e| wasm_bindgen::JsError::new(&format!("Lookup failed: {e}")))?;
            Ok(#ref_ident::from_ref(actor_ref))
        }

        #[wasm_bindgen(js_name = #bind_fn)]
        pub fn #bind_fn(name: &str, handle: &#ref_ident) -> Result<(), wasm_bindgen::JsError> {
            root_ctx().bind(name, handle.inner.clone())
                .map_err(|e| wasm_bindgen::JsError::new(&format!("Bind failed: {e}")))
        }

        #[cfg(feature = "remote")]
        #[wasm_bindgen(js_name = #lookup_fn)]
        pub async fn #lookup_fn(url: &str) -> Result<#ref_ident, wasm_bindgen::JsError> {
            let actor_ref = ActorRef::<#actor_ident>::lookup(url).await
                .map_err(|e| wasm_bindgen::JsError::new(&format!("Lookup failed: {e}")))?;
            Ok(#ref_ident::from_ref(actor_ref))
        }
    }
}
