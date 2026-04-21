//! `#[derive(TsType)]` — generates typed TypeScript interfaces for structs and enums.
//!
//! When applied to a struct, emits a `#[wasm_bindgen(typescript_custom_section)]`
//! with `export interface StructName { field: tsType; ... }`.
//! When applied to an enum, emits a discriminated union following serde's
//! externally-tagged convention.
//! Only active when both `ts` feature and `wasm32` target are enabled.
use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    Data, DataEnum, DeriveInput, Fields, Generics, Ident, PathArguments, Type, TypePath,
    parse_macro_input,
};

/// Entry point for `#[derive(TsType)]`.
pub(crate) fn derive_ts_type_impl(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident.to_string();
    let generic_params = collect_generic_params(&input.generics);
    let generic_decl = format_generic_decl(&generic_params);

    match &input.data {
        Data::Struct(s) => derive_struct(&name, &generic_decl, &s.fields, &input.ident),
        Data::Enum(e) => derive_enum(&name, &generic_decl, e, &input.ident),
        _ => syn::Error::new_spanned(
            &input.ident,
            "TsType can only be derived for structs and enums",
        )
        .to_compile_error()
        .into(),
    }
}

/// Map a `syn::Type` to a TypeScript type string (for use from ts.rs).
pub(crate) fn rust_type_to_ts_string(ty: &Type) -> String {
    rust_type_to_ts(ty)
}

/// Map a `syn::TypePath` to a TypeScript type string (for view types).
pub(crate) fn rust_type_to_ts_from_path(tp: &TypePath) -> String {
    rust_type_to_ts(&Type::Path(tp.clone()))
}

/// Map a Rust type to a TypeScript type string.
fn rust_type_to_ts(ty: &Type) -> String {
    match ty {
        Type::Path(TypePath { path, .. }) => {
            let Some(seg) = path.segments.last() else {
                return "any".to_string();
            };

            let ident = seg.ident.to_string();

            match ident.as_str() {
                "String" | "str" => "string".to_string(),
                "bool" => "boolean".to_string(),
                "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64"
                | "i128" | "isize" | "f32" | "f64" => "number".to_string(),
                "Vec" => {
                    let inner = extract_first_generic_arg(&seg.arguments);

                    format!("{}[]", inner)
                }
                "Option" => {
                    let inner = extract_first_generic_arg(&seg.arguments);

                    format!("{} | null", inner)
                }
                "HashMap" | "BTreeMap" => {
                    let (key, val) = extract_two_generic_args(&seg.arguments);

                    format!("Record<{}, {}>", key, val)
                }
                "Result" => {
                    let (ok, err) = extract_two_generic_args(&seg.arguments);

                    format!("{{ Ok: {} }} | {{ Err: {} }}", ok, err)
                }
                "ActorRef" => {
                    let inner = extract_first_generic_arg(&seg.arguments);

                    format!("{}Ref", inner)
                }
                other => other.to_string(),
            }
        }
        Type::Reference(r) => rust_type_to_ts(&r.elem),
        Type::Tuple(tuple) => {
            if tuple.elems.is_empty() {
                "void".to_string()
            } else {
                let elems: Vec<String> = tuple.elems.iter().map(rust_type_to_ts).collect();

                format!("[{}]", elems.join(", "))
            }
        }
        _ => "any".to_string(),
    }
}

/// Extract the first generic argument's TS representation.
fn extract_first_generic_arg(args: &PathArguments) -> String {
    let syn::PathArguments::AngleBracketed(ab) = args else {
        return "any".to_string();
    };

    let Some(syn::GenericArgument::Type(ty)) = ab.args.first() else {
        return "any".to_string();
    };

    rust_type_to_ts(ty)
}

/// Extract two generic arguments (e.g. for HashMap<K, V>).
fn extract_two_generic_args(args: &PathArguments) -> (String, String) {
    let syn::PathArguments::AngleBracketed(ab) = args else {
        return ("string".to_string(), "any".to_string());
    };

    let mut iter = ab.args.iter();

    let key = iter
        .next()
        .and_then(|a| match a {
            syn::GenericArgument::Type(ty) => Some(rust_type_to_ts(ty)),
            _ => None,
        })
        .unwrap_or_else(|| "string".to_string());
    let val = iter
        .next()
        .and_then(|a| match a {
            syn::GenericArgument::Type(ty) => Some(rust_type_to_ts(ty)),
            _ => None,
        })
        .unwrap_or_else(|| "any".to_string());

    (key, val)
}

/// Collect generic type parameter names (e.g. `<T, U>` → `["T", "U"]`).
fn collect_generic_params(generics: &Generics) -> Vec<String> {
    generics
        .type_params()
        .map(|tp| tp.ident.to_string())
        .collect()
}

/// Format generic parameter declaration for TS (e.g. `<T, U>`).
fn format_generic_decl(params: &[String]) -> String {
    if params.is_empty() {
        String::new()
    } else {
        format!("<{}>", params.join(", "))
    }
}

/// Generate TS type for a struct.
fn derive_struct(
    name: &str,
    generic_decl: &str,
    fields: &Fields,
    ident: &Ident,
) -> proc_macro::TokenStream {
    let ts_fields = match fields {
        Fields::Named(named) => named
            .named
            .iter()
            .map(|f| {
                let field_name = f.ident.as_ref().unwrap().to_string();
                let ts_ty = rust_type_to_ts(&f.ty);

                format!("  {}: {};", field_name, ts_ty)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Fields::Unnamed(unnamed) if unnamed.unnamed.len() == 1 => {
            let ts_ty = rust_type_to_ts(&unnamed.unnamed.first().unwrap().ty);
            let ts_section = format!("export type {}{} = {};", name, generic_decl, ts_ty);

            return generate_custom_section(&ts_section).into();
        }
        Fields::Unit => {
            let ts_section = format!("export type {}{} = null;", name, generic_decl);

            return generate_custom_section(&ts_section).into();
        }
        _ => {
            return syn::Error::new_spanned(
                ident,
                "TsType: tuple structs with multiple fields are not supported",
            )
            .to_compile_error()
            .into();
        }
    };

    let ts_section = format!(
        "export interface {}{} {{\n{}\n}}",
        name, generic_decl, ts_fields
    );

    generate_custom_section(&ts_section).into()
}

/// Generate TS type for an enum (serde externally-tagged convention).
fn derive_enum(
    name: &str,
    generic_decl: &str,
    data: &DataEnum,
    _ident: &Ident,
) -> proc_macro::TokenStream {
    let variants: Vec<String> = data
        .variants
        .iter()
        .map(|v| {
            let variant_name = v.ident.to_string();

            match &v.fields {
                Fields::Unit => format!("\"{}\"", variant_name),
                Fields::Named(named) => {
                    let fields: Vec<String> = named
                        .named
                        .iter()
                        .map(|f| {
                            let field_name = f.ident.as_ref().unwrap().to_string();
                            let ts_ty = rust_type_to_ts(&f.ty);

                            format!("{}: {}", field_name, ts_ty)
                        })
                        .collect();

                    format!("{{ {}: {{ {} }} }}", variant_name, fields.join("; "))
                }
                Fields::Unnamed(unnamed) => {
                    if unnamed.unnamed.len() == 1 {
                        let ts_ty = rust_type_to_ts(&unnamed.unnamed.first().unwrap().ty);

                        format!("{{ {}: {} }}", variant_name, ts_ty)
                    } else {
                        let elems: Vec<String> = unnamed
                            .unnamed
                            .iter()
                            .map(|f| rust_type_to_ts(&f.ty))
                            .collect();

                        format!("{{ {}: [{}] }}", variant_name, elems.join(", "))
                    }
                }
            }
        })
        .collect();

    let union = if variants.is_empty() {
        "never".to_string()
    } else {
        variants.join(" | ")
    };

    let ts_section = format!("export type {}{} = {};", name, generic_decl, union);

    generate_custom_section(&ts_section).into()
}

/// Emit a `typescript_custom_section` const.
fn generate_custom_section(ts_section: &str) -> TokenStream {
    quote! {
        #[cfg(all(feature = "ts", target_arch = "wasm32"))] #[allow(non_upper_case_globals)] const _
        : () = {
        #[::theta_ts::prelude::wasm_bindgen::prelude::wasm_bindgen(typescript_custom_section)] const
        TS_TYPE : &'static str = # ts_section; };
    }
}
