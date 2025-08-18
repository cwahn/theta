use serde::{Deserialize, Serialize};
use std::{any::Any, borrow::Cow};

use crate::actor::Actor;

pub type Ident = Cow<'static, [u8]>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nil;

pub(crate) fn panic_msg(payload: Box<dyn Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Ok(s) = payload.downcast::<String>() {
        *s
    } else {
        unreachable!("payload should be a string or &str")
    }
}

#[macro_export]
macro_rules! trace {
    ($($arg:tt)*) => {
        {#[cfg(feature = "tracing")] tracing::trace!($($arg)*);}
    };
}

#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
        {#[cfg(feature = "tracing")] tracing::debug!($($arg)*);}
    };
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        {#[cfg(feature = "tracing")] tracing::info!($($arg)*);}
    };
}

#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {
        {#[cfg(feature = "tracing")] tracing::warn!($($arg)*);}
    };
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {
        {#[cfg(feature = "tracing")] tracing::error!($($arg)*);}
    };
}

// Implementations

impl<T: Actor> From<&T> for Nil {
    fn from(_: &T) -> Self {
        Nil
    }
}
