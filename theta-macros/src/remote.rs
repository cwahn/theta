use proc_macro::TokenStream;
use quote::quote;
use syn::parse_macro_input;

// todo Implement attr macro to implement this as #[type_id("27bf12bd-73a6-4241-98df-ae2a0e37d3dd")]
pub(crate) fn type_id_attr_impl(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as syn::LitStr);
    let input = parse_macro_input!(input as syn::DeriveInput);

    match generate_type_id_attr_impl(&input, &args) {
        Ok(tokens) => TokenStream::from(tokens),
        Err(err) => TokenStream::from(err.to_compile_error()),
    }
}

fn generate_type_id_attr_impl(
    input: &syn::DeriveInput,
    args: &syn::LitStr,
) -> syn::Result<proc_macro2::TokenStream> {
    let type_name = &input.ident;
    let uuid_str = args.value();

    Ok(quote! {
        #input

        impl ::theta::remote::serde::GlobalType for #type_name {
            fn type_id(&self) -> ::theta::remote::serde::TypeId {
                ::uuid::uuid!(#uuid_str)
            }
        }
    })
}
