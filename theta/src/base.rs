//! Base types and utilities used throughout the framework.

use serde::{Deserialize, Serialize};
use std::{any::Any, borrow::Cow};

use crate::actor::Actor;

/// Actor identifier type for named bindings and lookups.
pub type Ident = Cow<'static, [u8]>;

/// A unit type representing "no value" or empty state.
///
/// This is commonly used as the `StateReport` type for actors that
/// don't need to report state to monitors.
///
/// # Example
///
/// ```
/// use theta::prelude::*;
///
/// #[derive(Debug, Clone, ActorArgs)]
/// struct MyActor;
///
/// #[actor("12345678-1234-5678-9abc-123456789abc")]
/// impl Actor for MyActor {}
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nil;

/// Extract panic message from panic payload for error reporting.
pub(crate) fn panic_msg(payload: Box<dyn Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Ok(s) = payload.downcast::<String>() {
        *s
    } else {
        unreachable!("payload should be a string or &str")
    }
}

#[cfg(feature = "tracing")]
#[macro_export]
macro_rules! trace {
    ($($arg:tt)*) => {
        {tracing::trace!($($arg)*);}
    };
}

#[cfg(not(feature = "tracing"))]
#[macro_export]
macro_rules! trace {
    ($($arg:tt)*) => {};
}

#[cfg(feature = "tracing")]
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
        {tracing::debug!($($arg)*);}
    };
}

#[cfg(not(feature = "tracing"))]
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {};
}

#[cfg(feature = "tracing")]
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        {tracing::info!($($arg)*);}
    };
}

#[cfg(not(feature = "tracing"))]
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {};
}

#[cfg(feature = "tracing")]
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {
        {tracing::warn!($($arg)*);}
    };
}

#[cfg(not(feature = "tracing"))]
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {};
}

#[cfg(feature = "tracing")]
#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {
        {tracing::error!($($arg)*);}
    };
}

#[cfg(not(feature = "tracing"))]
#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {};
}

// Implementations

impl<T: Actor> From<&T> for Nil {
    fn from(_: &T) -> Self {
        Nil
    }
}
