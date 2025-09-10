//! Base types and utilities used throughout the framework.

use serde::{Deserialize, Serialize};
use std::{any::Any, borrow::Cow};
use thiserror::Error;

use crate::{actor::Actor, context::LookupError};

/// Actor identifier type for named bindings and lookups.
pub type Ident = Cow<'static, [u8]>;

/// A unit type representing "no value" or empty state.
///
/// This is commonly used as the `View` type for actors that
/// don't need to update state to monitors.
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

/// Errors that can occur when monitoring actor status.
/// This is here in order to keep the inter-peer type does not depend on feature flags.
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum MonitorError {
    #[error(transparent)]
    LookupError(#[from] LookupError),
    #[error("failed to send signal")]
    SigSendError,
}

/// Extract panic message from panic payload for error updateing.
pub(crate) fn panic_msg(payload: Box<dyn Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Ok(s) = payload.downcast::<String>() {
        *s
    } else {
        unreachable!("payload should be a string or &str")
    }
}

// Implementations

impl<T: Actor> From<&T> for Nil {
    fn from(_: &T) -> Self {
        Nil
    }
}
