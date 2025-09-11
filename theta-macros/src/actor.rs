use proc_macro::TokenStream;
use proc_macro2::{Literal, Span, TokenStream as TokenStream2};
use quote::quote;
use syn::{
    Block, Expr, ExprClosure, Pat, ReturnType, Stmt, Token, Type, TypePath, Variant,
    parse_macro_input, parse_quote,
};

// Structure to parse actor macro arguments
#[derive(Debug, Clone)]
struct ActorArgs {
    uuid: syn::LitStr,
    snapshot: Option<Option<TypePath>>, // None = no snapshot, Some(None) = snapshot with default Self, Some(Some(type)) = explicit type
    feature: Option<syn::LitStr>,       // Optional feature flag to enable processing logic
}

impl syn::parse::Parse for ActorArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let uuid: syn::LitStr = input.parse()?;

        let mut snapshot = None;
        let mut feature = None;

        // Parse optional arguments
        while input.peek(Token![,]) {
            input.parse::<Token![,]>()?;

            if input.is_empty() {
                break;
            }

            if input.peek(syn::Ident) {
                let ident: syn::Ident = input.parse()?;
                match ident.to_string().as_str() {
                    "snapshot" => {
                        if input.peek(Token![=]) {
                            input.parse::<Token![=]>()?;
                            let snapshot_type: TypePath = input.parse()?;
                            snapshot = Some(Some(snapshot_type));
                        } else {
                            // Just `snapshot` without explicit type - default to Self
                            snapshot = Some(None);
                        }
                    }
                    "feature" => {
                        if input.peek(Token![=]) {
                            input.parse::<Token![=]>()?;
                            let feature_str: syn::LitStr = input.parse()?;
                            feature = Some(feature_str);
                        } else {
                            return Err(syn::Error::new_spanned(
                                ident,
                                "Expected 'feature = \"name\"'",
                            ));
                        }
                    }
                    _ => {
                        return Err(syn::Error::new_spanned(
                            ident,
                            "Expected 'snapshot' or 'feature'",
                        ));
                    }
                }
            }
        }

        Ok(ActorArgs {
            uuid,
            snapshot,
            feature,
        })
    }
}

pub(crate) fn actor_impl(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as ActorArgs);

    // First, try to parse the input to validate syntax and preserve error locations
    let input_tokens = TokenStream2::from(input);
    let input = match syn::parse2::<syn::ItemImpl>(input_tokens.clone()) {
        // If parsing fails, return the original error with preserved span
        Err(err) => return TokenStream::from(err.to_compile_error()),
        Ok(input) => input,
    };

    // Validate the input structure before proceeding with generation
    if let Err(err) = validate_actor_impl_structure(&input) {
        return TokenStream::from(err.to_compile_error());
    }

    match generate_actor_impl(input, &args) {
        Err(err) => TokenStream::from(err.to_compile_error()),
        Ok(tokens) => TokenStream::from(tokens),
    }
}

pub(crate) fn derive_actor_args_impl(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);

    match generate_actor_args_impl(&input) {
        Err(err) => TokenStream::from(err.to_compile_error()),
        Ok(tokens) => TokenStream::from(tokens),
    }
}

// Helper structures
#[derive(Debug, Clone)]
struct AsyncClosure {
    param_pattern: Pat,
    param_type: TypePath,
    return_type: Option<Type>,
    body: Block,
}

// Implementation functions

fn validate_actor_impl_structure(input: &syn::ItemImpl) -> syn::Result<()> {
    // Validate that it's an Actor trait impl
    let is_actor_impl = matches!(
        &input.trait_,
        Some((_bang, path, _for)) if path.segments.last().map(|s| s.ident == "Actor").unwrap_or(false)
    );
    if !is_actor_impl {
        return Err(syn::Error::new_spanned(
            &input.self_ty,
            "#[actor(...)] must be applied to `impl ::theta::actor::Actor for T { ... }`",
        ));
    }

    // Validate closures early to preserve their error spans
    for item in &input.items {
        if let syn::ImplItem::Const(const_item) = item
            && matches!(const_item.ident.to_string().as_str(), "_")
            && let syn::Expr::Block(block_expr) = &const_item.expr
        {
            validate_closures_in_block(&block_expr.block)?;
        }
    }

    Ok(())
}

fn validate_closures_in_block(block: &Block) -> syn::Result<()> {
    for stmt in &block.stmts {
        if let Stmt::Expr(expr, _) = stmt {
            validate_closure_expr(expr)?;
        }
    }
    Ok(())
}

fn validate_closure_expr(expr: &Expr) -> syn::Result<()> {
    match expr {
        Expr::Closure(closure) => {
            if closure.asyncness.is_some() {
                // Validate closure structure with preserved spans
                validate_async_closure(closure)?;
            }
        }
        _ => return Err(syn::Error::new_spanned(expr, "Expected async closures")),
    }
    Ok(())
}

fn validate_async_closure(closure: &ExprClosure) -> syn::Result<()> {
    if closure.inputs.len() != 1 {
        return Err(syn::Error::new_spanned(
            closure,
            "Message handler closure must have exactly one parameter",
        ));
    }

    let param = closure.inputs.first().unwrap();
    validate_param_pattern(param)?;

    // Validate that the body is a block expression
    match &*closure.body {
        Expr::Block(_) => {
            // Body is valid - the actual syntax within the block will be
            // validated by the Rust compiler with proper error locations
        }
        _ => {
            return Err(syn::Error::new_spanned(
                &*closure.body,
                "Closure body must be a block expression",
            ));
        }
    }

    Ok(())
}

fn validate_param_pattern(param: &Pat) -> syn::Result<()> {
    match param {
        Pat::Type(pat_type) => {
            if let Type::Path(_) = &*pat_type.ty {
                Ok(())
            } else {
                Err(syn::Error::new_spanned(
                    pat_type,
                    "Parameter type must be a path type",
                ))
            }
        }
        Pat::Struct(_) | Pat::TupleStruct(_) => Ok(()),
        _ => Err(syn::Error::new_spanned(
            param,
            "Parameter must be typed or destructuring pattern",
        )),
    }
}

fn generate_actor_args_impl(input: &syn::DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;

    let expanded = quote! {
        impl ::theta::actor::ActorArgs for #name {
            type Actor = Self;

            async fn initialize(
                ctx: ::theta::context::Context<Self::Actor>,
                cfg: &Self,
            ) -> Self::Actor {
                cfg.clone()
            }
        }

        impl ::core::convert::From<&#name> for #name {
            fn from(actor: &#name) -> Self {
                actor.clone()
            }
        }
    };

    Ok(expanded)
}

fn generate_actor_impl(mut input: syn::ItemImpl, args: &ActorArgs) -> syn::Result<TokenStream2> {
    use syn::{Expr, ImplItem};

    // Structure validation is already done in validate_actor_impl_structure

    // Analyze BEFORE mutating.
    let actor_type = extract_actor_type(&input)?;
    let async_closures = extract_async_closures_from_impl(&input)?;
    let view = extract_view(&input)?;

    let enum_ident = generate_enum_message_ident(&actor_type);
    let param_types = extract_message_types(&async_closures);
    let variant_idents = async_closures
        .iter()
        .map(|c| generate_enum_message_variant_ident(&c.param_type, enum_ident.span()))
        .collect::<Vec<_>>();

    let process_msg_impl_ts = generate_process_msg_impl(&variant_idents, &args.feature)?;
    let enum_message = generate_enum_message(&enum_ident, &variant_idents, &param_types)?;

    // Only generate FromTaggedBytes impl if remote feature is enabled
    #[cfg(feature = "remote")]
    let from_tagged_bytes_impl =
        generate_from_tagged_bytes_impl(&enum_ident, &param_types, &variant_idents)?;
    #[cfg(not(feature = "remote"))]
    let from_tagged_bytes_impl = quote! {};

    let message_impls = generate_message_impls(&actor_type, &async_closures, &args.feature)?;
    let into_impls = generate_into_impls(&enum_ident, &param_types, &variant_idents)?;

    // Generate PersistentActor implementation if snapshot attribute is present
    let persistent_actor_impl = if let Some(snapshot_type_opt) = &args.snapshot {
        let snapshot_type = match snapshot_type_opt {
            Some(explicit_type) => explicit_type.clone(),
            None => {
                // Default to Self if snapshot is specified without a type
                let self_path: TypePath = parse_quote!(#actor_type);
                self_path
            }
        };
        generate_persistent_actor_impl(&actor_type, &snapshot_type)?
    } else {
        quote! {}
    };

    // Consume only the scratchpad const `_`.
    input.items.retain(|it| {
        !matches!(it, ImplItem::Const(c) if c.ident == "_" && matches!(c.expr, Expr::Block(_)))
    });

    // todo Reduce iteration
    // Borrow-safe helpers that DO NOT capture `input`
    fn has_assoc_type(items: &[ImplItem], name: &str) -> bool {
        items
            .iter()
            .any(|it| matches!(it, ImplItem::Type(t) if t.ident == name))
    }
    fn has_method(items: &[ImplItem], name: &str) -> bool {
        items
            .iter()
            .any(|it| matches!(it, ImplItem::Fn(f) if f.sig.ident == name))
    }
    fn has_const(items: &[ImplItem], name: &str) -> bool {
        items
            .iter()
            .any(|it| matches!(it, ImplItem::Const(c) if c.ident == name))
    }

    // Insert pieces only if missing. Each call borrows immutably for the call only.
    if !has_assoc_type(&input.items, "Msg") {
        input.items.push(parse_quote!( type Msg = #enum_ident; ));
    }

    if !has_assoc_type(&input.items, "View") {
        input.items.push(parse_quote!( type View = #view; ));
    } else if !has_method(&input.items, "hash_code") {
        let hash_code_impl = generate_hash_code_impl()?;
        input.items.push(hash_code_impl);
    }

    if !has_method(&input.items, "process_msg") {
        let item_fn: ImplItem = syn::parse2(process_msg_impl_ts)?;
        input.items.push(item_fn);
    }

    if !has_const(&input.items, "IMPL_ID") {
        // Only generate IMPL_ID if remote feature is enabled in macro crate
        #[cfg(feature = "remote")]
        {
            let uuid = &args.uuid;
            input.items.push(parse_quote!(
                const IMPL_ID: ::theta::remote::base::ActorTypeId =
                    ::theta::__private::uuid::uuid!(#uuid);
            ));
        }

        // Suppress unused variable warning when remote feature is disabled
        #[cfg(not(feature = "remote"))]
        {
            let _uuid = &args.uuid;
        }
    }

    // Emit: one mutated Actor impl + the top-level generated artifacts.
    Ok(quote! {
        #input

        #enum_message
        #from_tagged_bytes_impl
        #(#message_impls)*
        #(#into_impls)*
        #persistent_actor_impl
    })
}

fn extract_actor_type(input: &syn::ItemImpl) -> syn::Result<syn::Ident> {
    if let Type::Path(type_path) = &*input.self_ty {
        if let Some(segment) = type_path.path.segments.last() {
            Ok(segment.ident.clone())
        } else {
            Err(syn::Error::new_spanned(
                &input.self_ty,
                "Invalid actor type",
            ))
        }
    } else {
        Err(syn::Error::new_spanned(
            &input.self_ty,
            "Actor type must be a path",
        ))
    }
}

fn extract_async_closures_from_impl(input: &syn::ItemImpl) -> syn::Result<Vec<AsyncClosure>> {
    let mut closures = Vec::new();

    for item in &input.items {
        if let syn::ImplItem::Const(const_item) = item
            && matches!(const_item.ident.to_string().as_str(), "_")
            && let syn::Expr::Block(block_expr) = &const_item.expr
        {
            extract_closures_from_block(&block_expr.block, &mut closures)?;
        }
    }

    Ok(closures)
}

fn extract_view(input: &syn::ItemImpl) -> syn::Result<TypePath> {
    for item in &input.items {
        if let syn::ImplItem::Type(type_item) = item
            && type_item.ident == "View"
        {
            if let Type::Path(type_path) = &type_item.ty {
                return Ok(type_path.clone());
            } else {
                return Err(syn::Error::new_spanned(
                    &type_item.ty,
                    "View must be a path type",
                ));
            }
        }
    }

    Ok(parse_quote!(::theta::base::Nil))
}

fn extract_closures_from_block(block: &Block, closures: &mut Vec<AsyncClosure>) -> syn::Result<()> {
    for stmt in &block.stmts {
        if let Stmt::Expr(expr, _) = stmt {
            extract_closures_from_expr(expr, closures)?;
        }
    }
    Ok(())
}

fn extract_closures_from_expr(expr: &Expr, closures: &mut Vec<AsyncClosure>) -> syn::Result<()> {
    match expr {
        Expr::Closure(closure) => {
            if closure.asyncness.is_some() {
                let async_closure = parse_async_closure(closure)?;
                closures.push(async_closure);
            }
        }
        _ => {
            return Err(syn::Error::new_spanned(expr, "Expected async closures"));
        }
    }
    Ok(())
}

fn parse_async_closure(closure: &ExprClosure) -> syn::Result<AsyncClosure> {
    if closure.inputs.len() != 1 {
        return Err(syn::Error::new_spanned(
            closure,
            "Message handler closure must have exactly one parameter",
        ));
    }

    let param = closure.inputs.first().unwrap();
    let (param_type, param_pattern) = extract_type_and_pattern(param)?;

    let return_type = match &closure.output {
        ReturnType::Type(_, ty) => Some((**ty).clone()),
        ReturnType::Default => None,
    };

    let body = match &*closure.body {
        Expr::Block(block_expr) => block_expr.block.clone(),
        _ => {
            return Err(syn::Error::new_spanned(
                &*closure.body,
                "Closure body must be a block expression",
            ));
        }
    };

    Ok(AsyncClosure {
        param_pattern,
        param_type,
        return_type,
        body,
    })
}

fn extract_type_and_pattern(param: &Pat) -> syn::Result<(TypePath, Pat)> {
    match param {
        Pat::Type(pat_type) => {
            if let Type::Path(type_path) = &*pat_type.ty {
                Ok((type_path.clone(), (*pat_type.pat).clone()))
            } else {
                Err(syn::Error::new_spanned(
                    pat_type,
                    "Parameter type must be a path type",
                ))
            }
        }
        Pat::Struct(pat_struct) => Ok((
            TypePath {
                qself: pat_struct.qself.clone(),
                path: pat_struct.path.clone(),
            },
            param.clone(),
        )),
        Pat::TupleStruct(pat_tuple_struct) => Ok((
            TypePath {
                qself: pat_tuple_struct.qself.clone(),
                path: pat_tuple_struct.path.clone(),
            },
            param.clone(),
        )),
        _ => Err(syn::Error::new_spanned(
            param,
            "Parameter must be typed or destructuring pattern",
        )),
    }
}

fn extract_message_types(async_closures: &[AsyncClosure]) -> Vec<TypePath> {
    async_closures
        .iter()
        .map(|closure| closure.param_type.clone())
        .collect()
}

fn generate_process_msg_impl(
    message_enum_variant_idents: &[syn::Ident],
    feature: &Option<syn::LitStr>,
) -> syn::Result<TokenStream2> {
    // Generate match arms for each message type
    let match_arms: Vec<_> = message_enum_variant_idents
        .iter()
        .map(|variant_ident| {
            let tell_arm = feature_gated(feature, quote! { let _ = ::theta::message::Message::<Self>::process(self, ctx, m).await; });

            let ask_arm = feature_gated(feature, quote! {
                {
                    let any_ret = ::theta::message::Message::<Self>::process_to_any(self, ctx, m).await;
                    let _ = tx.send(any_ret);
                }
            });

            let base_arms = quote! {
                ::theta::message::Continuation::Nil => {
                    #tell_arm
                }
                ::theta::message::Continuation::Reply(tx) | ::theta::message::Continuation::Forward(tx) => {
                    #ask_arm
                }
            };

            // Remote arms only included if remote feature is enabled in macro crate

            let remote_arms = if cfg!(feature = "remote") {

                let error_handling = {
                    quote! { return ::theta::__private::log::error!("Failed to serialize message: {e}"); }
                };

                let forward_arm = feature_gated(feature, quote! {
                    {
                        let bytes = match ::theta::message::Message::<Self>::process_to_bytes(self, ctx, peer, m).await {
                            Ok(bytes) => bytes,
                            Err(e) => { #error_handling }
                        };
                        let _ = tx.send(bytes);
                    }
                });

                quote! {
                    ::theta::message::Continuation::BytesReply(peer, tx) | ::theta::message::Continuation::BytesForward(peer, tx) => {
                        #forward_arm
                    }
                }
            } else {
                quote! {}
            };

            let allow_unused = match feature {
                Some(feature) => quote! {#[cfg_attr(not(feature = #feature), allow(unused_variables))]},
                None => quote! { },
            };

            quote! {
                #allow_unused
                Self::Msg::#variant_ident(m) => {
                    match k {
                        #base_arms
                        #remote_arms
                    }
                }
            }
        })
        .collect();

    Ok(quote! {
        async fn process_msg(
            &mut self,
            ctx: ::theta::context::Context<Self>,
            msg: Self::Msg,
            k: ::theta::message::Continuation,
        ) -> () {
            match msg {
                #(#match_arms)*
            }
        }
    })
}

fn generate_enum_message(
    enum_ident: &syn::Ident,
    enum_message_variant_idents: &[syn::Ident],
    param_types: &[TypePath],
) -> syn::Result<TokenStream2> {
    let enum_message_variants =
        generate_enum_message_variants(enum_message_variant_idents, param_types)?;

    Ok(quote! {
        #[allow(non_camel_case_types)]
        #[derive(Debug, Clone, ::theta::__private::serde::Serialize, ::theta::__private::serde::Deserialize)]
        pub enum #enum_ident {
            #(#enum_message_variants),*
        }
    })
}

fn generate_enum_message_ident(name: &syn::Ident) -> syn::Ident {
    syn::Ident::new(&format!("{}__Msg", name), name.span())
}

fn generate_enum_message_variants(
    variant_idents: &[syn::Ident],
    param_types: &[TypePath],
) -> syn::Result<Vec<Variant>> {
    variant_idents
        .iter()
        .zip(param_types)
        .map(|(variant_ident, param_type)| {
            Ok(parse_quote! {
                #variant_ident(#param_type)
            })
        })
        .collect()
}

fn generate_enum_message_variant_ident(ty: &TypePath, span: Span) -> syn::Ident {
    let mut joined = String::new();
    for (i, seg) in ty.path.segments.iter().enumerate() {
        if i > 0 {
            joined.push(' ');
        }
        joined.push_str(&seg.ident.to_string());
    }

    let variant_name = format!("__{}", heck::AsUpperCamelCase(&joined));

    syn::Ident::new(&variant_name, span)
}

#[cfg(feature = "remote")]
fn generate_from_tagged_bytes_impl(
    enum_ident: &syn::Ident,
    param_types: &[TypePath],
    variant_idents: &[syn::Ident],
) -> syn::Result<TokenStream2> {
    let deserialize_fns: Vec<_> = param_types
        .iter()
        .enumerate()
        .map(|(i, param_type)| {
            let variant_ident = &variant_idents[i];
            quote! {
                |bytes| ::theta::__private::postcard::from_bytes::<#param_type>(bytes).map(|m| #enum_ident::#variant_ident(m))
            }
        })
        .collect();

    let deserialize_fns = quote! {
        const DESERIALIZE_FNS: &[fn(&[u8]) -> Result<#enum_ident, ::theta::__private::postcard::Error>] = &[
            |bytes| ::theta::__private::postcard::from_bytes::<#enum_ident>(bytes).map(|m| m.into()),
            #(#deserialize_fns),*
        ];
    };

    Ok(quote! {
        impl ::theta::remote::serde::FromTaggedBytes for #enum_ident {
            fn from(tag: ::theta::remote::base::Tag, bytes: &[u8]) -> Result<Self, ::theta::__private::postcard::Error> {
                #deserialize_fns

                let Some(deserialize_fn) = DESERIALIZE_FNS.get(tag as usize) else {
                    return Err(::theta::__private::postcard::Error::SerdeDeCustom);
                };

                deserialize_fn(bytes)
            }
        }
    })
}

fn generate_message_impls(
    actor_ident: &syn::Ident,
    async_closures: &[AsyncClosure],
    feature: &Option<syn::LitStr>,
) -> syn::Result<Vec<TokenStream2>> {
    async_closures
        .iter()
        .enumerate()
        .map(|(i, closure)| generate_single_message_impl(actor_ident, closure, i, feature))
        .collect()
}

fn generate_single_message_impl(
    actor_ident: &syn::Ident,
    closure: &AsyncClosure,
    index: usize,
    feature: &Option<syn::LitStr>,
) -> syn::Result<TokenStream2> {
    let param_type = &closure.param_type;
    let param_pattern = &closure.param_pattern;
    let stmts = replace_self_with_state(&closure.body).stmts;

    let return_type = match &closure.return_type {
        Some(ty) => quote! { #ty },
        None => quote! {()},
    };

    // Only generate TAG if remote feature is enabled in macro crate
    #[cfg(feature = "remote")]
    let tag_const = {
        let idx = Literal::u32_suffixed(index as u32);
        quote! {
            const TAG: ::theta::remote::base::Tag = #idx;
        }
    };

    #[cfg(not(feature = "remote"))]
    let tag_const = {
        let _idx = Literal::u32_suffixed(index as u32);
        quote! {}
    };

    let feature_gated_body = feature_gated(
        feature,
        quote! {
            #(#stmts)*
        },
    );

    Ok(quote! {
        impl ::theta::message::Message<#actor_ident> for #param_type {
            type Return = #return_type;

            #tag_const

            fn process(
                state: &mut #actor_ident,
                ctx: ::theta::context::Context<#actor_ident>,
                #param_pattern: Self,
            ) -> impl ::std::future::Future<Output = Self::Return> + Send {
                async move {
                    // // #body
                    // #(#stmts)*
                    #feature_gated_body
                }
            }
        }
    })
}

fn replace_self_with_state(block: &Block) -> Block {
    use proc_macro2::{TokenStream, TokenTree};
    use syn::fold::{Fold, fold_expr, fold_stmt};

    struct SelfReplacer;

    impl SelfReplacer {
        fn replace_tokens_in_stream(tokens: TokenStream) -> TokenStream {
            let mut result = TokenStream::new();
            let tokens_iter = tokens.into_iter().peekable();

            for token in tokens_iter {
                match token {
                    TokenTree::Ident(ident) if ident == "self" => {
                        result.extend(std::iter::once(TokenTree::Ident(proc_macro2::Ident::new(
                            "state",
                            ident.span(),
                        ))));
                    }
                    TokenTree::Group(group) => {
                        let replaced_stream =
                            SelfReplacer::replace_tokens_in_stream(group.stream());
                        result.extend(std::iter::once(TokenTree::Group(proc_macro2::Group::new(
                            group.delimiter(),
                            replaced_stream,
                        ))));
                    }
                    other => {
                        result.extend(std::iter::once(other));
                    }
                }
            }
            result
        }
    }

    impl Fold for SelfReplacer {
        fn fold_stmt(&mut self, stmt: syn::Stmt) -> syn::Stmt {
            match stmt {
                syn::Stmt::Macro(mut stmt_macro) => {
                    // Handle macro statements (like println!)
                    stmt_macro.mac.tokens =
                        SelfReplacer::replace_tokens_in_stream(stmt_macro.mac.tokens);
                    syn::Stmt::Macro(stmt_macro)
                }
                other => fold_stmt(self, other),
            }
        }

        fn fold_expr(&mut self, expr: Expr) -> Expr {
            match expr {
                // Replace `self` with `state`
                Expr::Path(mut expr_path) if expr_path.path.is_ident("self") => {
                    expr_path.path = parse_quote!(state);
                    Expr::Path(expr_path)
                }
                // Replace `self.field` with `state.field`
                Expr::Field(mut expr_field) => {
                    expr_field.base = Box::new(self.fold_expr(*expr_field.base));
                    Expr::Field(expr_field)
                }
                // Replace `self.method()` with `state.method()`
                Expr::MethodCall(mut expr_method_call) => {
                    expr_method_call.receiver =
                        Box::new(self.fold_expr(*expr_method_call.receiver));
                    expr_method_call.args = expr_method_call
                        .args
                        .into_iter()
                        .map(|arg| self.fold_expr(arg))
                        .collect();
                    Expr::MethodCall(expr_method_call)
                }
                // Handle macro expressions
                Expr::Macro(mut expr_macro) => {
                    expr_macro.mac.tokens =
                        SelfReplacer::replace_tokens_in_stream(expr_macro.mac.tokens);
                    Expr::Macro(expr_macro)
                }
                // Continue folding for other expressions
                other => fold_expr(self, other),
            }
        }
    }

    let mut replacer = SelfReplacer;
    syn::fold::fold_block(&mut replacer, block.clone())
}

fn generate_into_impls(
    enum_ident: &syn::Ident,
    param_types: &[TypePath],
    variant_idents: &[syn::Ident],
) -> syn::Result<Vec<TokenStream2>> {
    param_types
        .iter()
        .zip(variant_idents)
        .map(|(param_type, variant_ident)| {
            Ok(quote! {
                impl From<#param_type> for #enum_ident {
                    fn from(msg: #param_type) -> Self {
                        Self::#variant_ident(msg)
                    }
                }
            })
        })
        .collect()
}

fn generate_persistent_actor_impl(
    actor_type: &syn::Ident,
    snapshot_type: &TypePath,
) -> syn::Result<TokenStream2> {
    Ok(quote! {
        impl ::theta::persistence::persistent_actor::PersistentActor for #actor_type {
            type Snapshot = #snapshot_type;
        }
    })
}

fn feature_gated(feature: &Option<syn::LitStr>, token: TokenStream2) -> TokenStream2 {
    if let Some(feature_name) = feature {
        quote! {
            #[cfg(feature = #feature_name)]
            {#token}
            #[cfg(not(feature = #feature_name))]
            ::std::unimplemented!("available with '#feature_name' feature")
        }
    } else {
        token
    }
}

fn generate_hash_code_impl() -> syn::Result<syn::ImplItem> {
    Ok(parse_quote! {
        fn hash_code(&self) -> u64 {
            let mut hasher = ::theta::__private::rustc_hash::FxHasher::default();
            ::std::hash::Hash::hash(self, &mut hasher);
            ::std::hash::Hasher::finish(&hasher)
        }
    })
}
