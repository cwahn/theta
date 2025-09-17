//! Logging macros for conditional tracing support.
//!
//! These proc macros generate macro_rules! definitions that conditionally
//! use the tracing crate when the "tracing" feature is enabled.

use proc_macro::TokenStream;
use quote::quote;

/// Generate logging macros that conditionally use tracing.
/// This is a function-like macro that generates all logging macros at once.
#[proc_macro]
pub fn generate_logging_macros(_input: TokenStream) -> TokenStream {
    let tokens = quote! {
    macro_rules! trace {
            ($($arg:tt)*) => {
        ::theta::__private::tracing::trace!($($arg)*)
            };
        }

    macro_rules! debug {
            ($($arg:tt)*) => {
        ::theta::__private::tracing::debug!($($arg)*)
            };
        }

    macro_rules! info {
            ($($arg:tt)*) => {
        ::theta::__private::tracing::info!($($arg)*)
            };
        }

    macro_rules! warn {
            ($($arg:tt)*) => {
        ::theta::__private::tracing::warn!($($arg)*)
            };
        }

    macro_rules! error {
            ($($arg:tt)*) => {
        ::theta::__private::tracing::error!($($arg)*)
            };
        }

        pub(crate) use trace;
        pub(crate) use debug;
        pub(crate) use info;
        pub(crate) use warn;
        pub(crate) use error;
    };

    tokens.into()
}
