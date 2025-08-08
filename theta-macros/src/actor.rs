use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::{
    Block, Expr, ExprClosure, Pat, ReturnType, Stmt, Type, TypePath, Variant, parse_macro_input,
    parse_quote,
};

pub(crate) fn intention_impl(input: TokenStream) -> TokenStream {
    let body: TokenStream2 = input.into();

    let expanded = quote! {
        async fn __process_msg(&mut self, ctx: Context<Self>) {
            #body
        }
    };
    TokenStream::from(expanded)
}

pub(crate) fn actor_impl(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as syn::LitStr);
    let input = parse_macro_input!(input as syn::ItemImpl);

    match generate_actor_impl(input, &args) {
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

// Helper structures
#[derive(Debug, Clone)]
struct AsyncClosure {
    param_pattern: Pat,
    param_type: TypePath,
    return_type: Option<Type>,
    body: Block,
}

// Implementation functions

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

fn generate_actor_impl(input: syn::ItemImpl, args: &syn::LitStr) -> syn::Result<TokenStream2> {
    let input = expand_intention_macros(input)?;
    let actor_type = extract_actor_type(&input)?;
    let process_msg_fn = find_process_msg_function(&input)?;
    let async_closures = extract_async_closures_from_function(process_msg_fn)?;
    let state_report = extract_state_report(&input)?;

    let enum_ident = generate_enum_message_ident(&actor_type);

    let param_types = extract_message_types(&async_closures);
    let variant_idents = async_closures
        .iter()
        .map(|closure| generate_enum_message_variant_ident(&closure.param_type, enum_ident.span()))
        .collect::<Vec<_>>();

    let process_msg_impl = generate_process_msg_impl(&variant_idents)?;

    let enum_message = generate_enum_message(&enum_ident, &variant_idents, &param_types)?;
    let message_impls = generate_message_impls(&actor_type, &async_closures)?;
    let into_impls = generate_into_impls(&enum_ident, &param_types, &variant_idents)?;

    Ok(quote! {

        impl ::theta::actor::Actor for #actor_type {
            type Msg = #enum_ident;
            type StateReport = #state_report;

            #process_msg_impl

            const __IMPL_ID: ::theta::base::ImplId = ::uuid::uuid!(#args);
        }

        #enum_message
        #(#message_impls)*
        #(#into_impls)*
    })
}

fn expand_intention_macros(mut input: syn::ItemImpl) -> syn::Result<syn::ItemImpl> {
    for item in &mut input.items {
        if let syn::ImplItem::Macro(macro_item) = item {
            if macro_item.mac.path.is_ident("intention") {
                // Manually expand the intention! macro
                let tokens = &macro_item.mac.tokens;

                let expanded_fn = parse_quote!(
                    async fn __process_msg(&mut self, ctx: Context<Self>) {
                        #tokens
                    }
                );

                // Replace the macro with the expanded function
                *item = syn::ImplItem::Fn(expanded_fn);
            }
        }
    }
    Ok(input)
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

fn find_process_msg_function(input: &syn::ItemImpl) -> syn::Result<&syn::ImplItemFn> {
    for item in &input.items {
        if let syn::ImplItem::Fn(fn_item) = item {
            if fn_item.sig.ident == "__process_msg" {
                return Ok(fn_item);
            }
        }
    }
    Err(syn::Error::new_spanned(
        input,
        "Could not find __process_msg function",
    ))
}

fn extract_async_closures_from_function(
    fn_item: &syn::ImplItemFn,
) -> syn::Result<Vec<AsyncClosure>> {
    let mut closures = Vec::new();
    extract_closures_from_block(&fn_item.block, &mut closures)?;
    Ok(closures)
}

fn extract_state_report(input: &syn::ItemImpl) -> syn::Result<TypePath> {
    for item in &input.items {
        if let syn::ImplItem::Type(type_item) = item {
            if type_item.ident == "StateReport" {
                if let Type::Path(type_path) = &type_item.ty {
                    return Ok(type_path.clone());
                } else {
                    return Err(syn::Error::new_spanned(
                        &type_item.ty,
                        "StateReport must be a path type",
                    ));
                }
            }
        }
    }

    Ok(parse_quote!(::theta::actor::Nil))
}

fn extract_closures_from_block(block: &Block, closures: &mut Vec<AsyncClosure>) -> syn::Result<()> {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Expr(expr, _) => {
                extract_closures_from_expr(expr, closures)?;
            }
            _ => {}
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
) -> syn::Result<TokenStream2> {
    // Generate match arms for each message type
    let match_arms: Vec<_> = message_enum_variant_idents
        .iter()
        .map(|variant_ident| {
            quote! {
                Self::Msg::#variant_ident(m) => {
                    if k.is_nil() {
                        let _ = ::theta::message::Message::<Self>::process(self, ctx, m).await;
                    } else {
                        let any_ret = ::theta::message::Message::<Self>::process_to_any(self, ctx, m).await;
                        k.send(any_ret);
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
    // async_closures: &[AsyncClosure],
    enum_message_variant_idents: &[syn::Ident],
    param_types: &[TypePath],
) -> syn::Result<TokenStream2> {
    let enum_message_variants =
        generate_enum_message_variants(enum_message_variant_idents, param_types)?;

    Ok(quote! {
        #[derive(Debug, Clone, ::serde::Serialize, ::serde::Deserialize)]
        pub enum #enum_ident {
            #(#enum_message_variants),*
        }
    })
}

fn generate_enum_message_ident(name: &syn::Ident) -> syn::Ident {
    syn::Ident::new(&format!("__Generated{}Msg", name), name.span())
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

fn generate_message_impls(
    actor_ident: &syn::Ident,
    async_closures: &[AsyncClosure],
) -> syn::Result<Vec<TokenStream2>> {
    async_closures
        .iter()
        .map(|closure| generate_single_message_impl(actor_ident, closure))
        .collect()
}

fn generate_single_message_impl(
    actor_ident: &syn::Ident,
    closure: &AsyncClosure,
) -> syn::Result<TokenStream2> {
    let param_type = &closure.param_type;
    let param_pattern = &closure.param_pattern;
    let body = replace_self_with_state(&closure.body);

    let return_type = match &closure.return_type {
        Some(ty) => quote! { #ty },
        None => quote! {()},
    };

    Ok(quote! {
        impl ::theta::message::Message<#actor_ident> for #param_type {
            type Return = #return_type;

            fn process(
                state: &mut #actor_ident,
                ctx: ::theta::context::Context<#actor_ident>,
                #param_pattern: Self,
            ) -> impl ::std::future::Future<Output = Self::Return> + Send {
                async move {
                    #body
                }
            }
        }
    })
}

fn replace_self_with_state(block: &Block) -> Block {
    use syn::fold::{Fold, fold_expr};

    struct SelfReplacer;

    impl Fold for SelfReplacer {
        fn fold_expr(&mut self, expr: Expr) -> Expr {
            match expr {
                // Replace `self` with `state`
                Expr::Path(mut expr_path) if expr_path.path.is_ident("self") => {
                    expr_path.path = parse_quote!(state);
                    Expr::Path(expr_path)
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
