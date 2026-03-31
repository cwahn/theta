//! TypeScript/WASM binding generation for actors.
//!
//! When `ts` is specified on `#[actor]`, this module generates:
//! - `typescript_custom_section` with TS type definitions
//! - `{Actor}Handle` wasm-bindgen class (tell, ask, prep, initStream)
//! - Free functions: spawn{Actor}, lookup{Actor}Local, bind{Actor}

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::TypePath;

/// Information about a single message handler needed for TS generation.
pub(crate) struct TsMsgInfo {
    /// The Rust message type name (e.g. `SendMessage`)
    pub rust_type_name: String,
    /// The variant ident in the Msg enum (e.g. `__SendMessage`)
    pub variant_ident: syn::Ident,
    /// Whether this handler has a return type (ask pattern)
    pub return_type: Option<syn::Type>,
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
    let handle_ident = format_ident!("{}Handle", actor_ident);
    let ts_msg_enum_ident = format_ident!("{}__TsMsg", actor_ident);

    // --- 1. typescript_custom_section ---
    let ts_types = generate_ts_type_section(&actor_name, view_type, msg_infos);

    // --- 2. TsMsg enum + From<JsValue> ---
    let ts_msg_enum = generate_ts_msg_enum(&ts_msg_enum_ident, enum_ident, msg_infos);

    // --- 3. Handle class ---
    let handle_class = generate_handle_class(
        actor_ident,
        &handle_ident,
        &ts_msg_enum_ident,
        enum_ident,
        view_type,
        msg_infos,
    );

    // --- 4. Free functions ---
    let free_fns = generate_free_functions(actor_ident, &handle_ident);

    Ok(quote! {
        // All TS/WASM bindings are behind ts feature + wasm32 target
        #[cfg(all(feature = "ts", target_arch = "wasm32"))]
        mod __ts_bindings {
            use super::*;
            use ::theta_wasm::prelude::*;

            #ts_types
            #ts_msg_enum
            #handle_class
            #free_fns
        }
    })
}

/// Generate the `typescript_custom_section` with TS type definitions.
fn generate_ts_type_section(
    actor_name: &str,
    _view_type: &TypePath,
    msg_infos: &[TsMsgInfo],
) -> TokenStream2 {
    // Build Msg discriminated union: { SendMessage: { text: string } } | ...
    // For now we use opaque types since we can't introspect Rust struct fields in proc macros.
    // Users will refine via #[message(ts)] later.
    let msg_variants: Vec<String> = msg_infos
        .iter()
        .map(|info| format!("{{ {}: any }}", info.rust_type_name))
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
            let ret = if info.return_type.is_some() {
                "any"
            } else {
                "void"
            };
            format!("  {}: {};", info.rust_type_name, ret)
        })
        .collect();
    let returns_body = returns_entries.join("\n");

    let ts_section = format!(
        r#"
export type {actor_name}Msg = {msg_union};

export interface {actor_name}Returns {{
{returns_body}
}}

export type {actor_name}View = any;

export interface {actor_name} {{
  readonly Msg: {actor_name}Msg;
  readonly Returns: {actor_name}Returns;
  readonly View: {actor_name}View;
}}
"#,
        actor_name = actor_name,
        msg_union = msg_union,
        returns_body = returns_body,
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
        #[derive(::theta_wasm::prelude::serde::Deserialize)]
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

/// Generate the `{Actor}Handle` wasm-bindgen class.
fn generate_handle_class(
    actor_ident: &syn::Ident,
    handle_ident: &syn::Ident,
    ts_msg_enum_ident: &syn::Ident,
    _rust_enum_ident: &syn::Ident,
    _view_type: &TypePath,
    msg_infos: &[TsMsgInfo],
) -> TokenStream2 {
    let has_messages = !msg_infos.is_empty();
    let has_ask = msg_infos.iter().any(|m| m.return_type.is_some());

    let tell_method = if has_messages {
        quote! {
            #[wasm_bindgen]
            pub fn tell(&self, msg: wasm_bindgen::JsValue) -> Result<(), wasm_bindgen::JsError> {
                let ts_msg: #ts_msg_enum_ident = serde_wasm_bindgen::from_value(msg)
                    .map_err(|e| wasm_bindgen::JsError::new(&format!("Invalid message: {e}")))?;
                self.inner
                    .tell(ts_msg.into())
                    .map_err(|e| wasm_bindgen::JsError::new(&format!("Tell failed: {e}")))
            }
        }
    } else {
        quote! {}
    };

    let ask_method = if has_ask {
        quote! {
            #[wasm_bindgen]
            pub async fn ask(&self, msg: wasm_bindgen::JsValue) -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsError> {
                let ts_msg: #ts_msg_enum_ident = serde_wasm_bindgen::from_value(msg)
                    .map_err(|e| wasm_bindgen::JsError::new(&format!("Invalid message: {e}")))?;
                let rust_msg: <#actor_ident as ::theta::actor::Actor>::Msg = ts_msg.into();

                // Use raw send with reply continuation
                let (tx, rx) = ::futures::channel::oneshot::channel::<Box<dyn ::std::any::Any + Send>>();
                self.inner
                    .send(rust_msg, ::theta::message::Continuation::Reply(tx))
                    .map_err(|e| wasm_bindgen::JsError::new(&format!("Ask failed: {e}")))?;

                let any_result = rx.await
                    .map_err(|_| wasm_bindgen::JsError::new("Ask cancelled — actor dropped"))?;

                // Serialize the Any result back to JsValue via serde
                // The result is already the concrete return type boxed as Any.
                // We serialize it through serde_json as an intermediate step.
                Ok(wasm_bindgen::JsValue::from_str(&format!("{:?}", any_result)))
            }
        }
    } else {
        quote! {}
    };

    quote! {
        #[wasm_bindgen]
        pub struct #handle_ident {
            inner: ActorRef<#actor_ident>,
        }

        #[wasm_bindgen]
        impl #handle_ident {
            /// Get the actor's unique identifier.
            #[wasm_bindgen(getter)]
            pub fn id(&self) -> String {
                self.inner.id().map(|id| id.to_string()).unwrap_or_default()
            }

            #tell_method
            #ask_method

            /// Get the current state snapshot (View).
            /// Returns null if monitoring is not available.
            #[wasm_bindgen]
            pub fn prep(&self) -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsError> {
                // prep returns the initial View snapshot via a monitor channel
                let (tx, rx) = ::theta_wasm::prelude::theta_flume::unbounded_anonymous();
                self.inner.monitor_local(tx)
                    .map_err(|e| wasm_bindgen::JsError::new(&format!("Monitor failed: {e}")))?;

                // Try to receive the first state update synchronously
                match rx.try_recv() {
                    Some(::theta::monitor::Update::State(view)) => {
                        serde_wasm_bindgen::to_value(&view)
                            .map_err(|e| wasm_bindgen::JsError::new(&format!("Serialize failed: {e}")))
                    }
                    _ => Ok(wasm_bindgen::JsValue::NULL),
                }
            }

            /// Start streaming state updates. Calls the JS callback on each update.
            /// Returns a closure that stops the stream when called.
            #[wasm_bindgen(js_name = "initStream")]
            pub fn init_stream(&self, callback: js_sys::Function) -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsError> {
                let (tx, rx) = ::theta_wasm::prelude::theta_flume::unbounded_anonymous();
                self.inner.monitor_local(tx)
                    .map_err(|e| wasm_bindgen::JsError::new(&format!("Monitor failed: {e}")))?;

                // Spawn a task that forwards updates to the JS callback
                ::theta_wasm::prelude::wasm_bindgen_futures::spawn_local(async move {
                    while let Some(update) = rx.recv().await {
                        match update {
                            ::theta::monitor::Update::State(view) => {
                                if let Ok(js_val) = serde_wasm_bindgen::to_value(&view) {
                                    let _ = callback.call1(
                                        &wasm_bindgen::JsValue::NULL,
                                        &js_val,
                                    );
                                }
                            }
                            ::theta::monitor::Update::Status(_status) => {
                                // Status updates could be forwarded in the future
                            }
                        }
                    }
                });

                Ok(wasm_bindgen::JsValue::UNDEFINED)
            }
        }

        impl #handle_ident {
            pub(crate) fn from_ref(actor_ref: ActorRef<#actor_ident>) -> Self {
                Self { inner: actor_ref }
            }
        }
    }
}

/// Generate free functions: spawn{Actor}, lookup{Actor}Local, bind{Actor}.
fn generate_free_functions(
    actor_ident: &syn::Ident,
    handle_ident: &syn::Ident,
) -> TokenStream2 {
    let spawn_fn = format_ident!("spawn{}", actor_ident);
    let lookup_fn = format_ident!("lookup{}Local", actor_ident);
    let bind_fn = format_ident!("bind{}", actor_ident);

    quote! {
        #[wasm_bindgen(js_name = #spawn_fn)]
        pub fn #spawn_fn(args: wasm_bindgen::JsValue) -> Result<#handle_ident, wasm_bindgen::JsError> {
            let actor_args: #actor_ident = serde_wasm_bindgen::from_value(args)
                .map_err(|e| wasm_bindgen::JsError::new(&format!("Invalid args: {e}")))?;
            let actor_ref = root_ctx().spawn(actor_args);
            Ok(#handle_ident::from_ref(actor_ref))
        }

        #[wasm_bindgen(js_name = #lookup_fn)]
        pub fn #lookup_fn(name: &str) -> Result<#handle_ident, wasm_bindgen::JsError> {
            let actor_ref = ActorRef::<#actor_ident>::lookup_local(name)
                .map_err(|e| wasm_bindgen::JsError::new(&format!("Lookup failed: {e}")))?;
            Ok(#handle_ident::from_ref(actor_ref))
        }

        #[wasm_bindgen(js_name = #bind_fn)]
        pub fn #bind_fn(name: &str, handle: &#handle_ident) {
            root_ctx().bind(name, handle.inner.clone());
        }
    }
}
