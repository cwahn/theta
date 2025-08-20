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
        #[cfg(feature = "tracing")]
        macro_rules! trace {
            ($($arg:tt)*) => {
                tracing::trace!($($arg)*)
            };
        }

        #[cfg(not(feature = "tracing"))]
        macro_rules! trace {
            ($($arg:tt)*) => {};
        }

        #[cfg(feature = "tracing")]
        macro_rules! debug {
            ($($arg:tt)*) => {
                tracing::debug!($($arg)*)
            };
        }

        #[cfg(not(feature = "tracing"))]
        macro_rules! debug {
            ($($arg:tt)*) => {};
        }

        #[cfg(feature = "tracing")]
        macro_rules! info {
            ($($arg:tt)*) => {
                tracing::info!($($arg)*)
            };
        }

        #[cfg(not(feature = "tracing"))]
        macro_rules! info {
            ($($arg:tt)*) => {};
        }

        #[cfg(feature = "tracing")]
        macro_rules! warn {
            ($($arg:tt)*) => {
                tracing::warn!($($arg)*)
            };
        }

        #[cfg(not(feature = "tracing"))]
        macro_rules! warn {
            ($($arg:tt)*) => {};
        }

        #[cfg(feature = "tracing")]
        macro_rules! error {
            ($($arg:tt)*) => {
                tracing::error!($($arg)*)
            };
        }

        #[cfg(not(feature = "tracing"))]
        macro_rules! error {
            ($($arg:tt)*) => {};
        }

        pub(crate) use trace;
        pub(crate) use debug;
        pub(crate) use info;
        pub(crate) use warn;
        pub(crate) use error;
    };

    tokens.into()
}
