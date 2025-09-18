//! Base types and utilities used throughout the framework.

use serde::{Deserialize, Serialize};
use std::{
    any::Any,
    fmt::{Debug, Display},
};
use thiserror::Error;

use crate::{actor::Actor, context::LookupError};

/// Actor identifier type for named bindings and lookups.
///
/// Intentionally restrictive design has two reasons:
/// 1. To support allocation-restricted targets
/// 2. To recommend restrained use of global binding and lookups as they undermine 'locality' and 'security' of actor system
pub type Ident = [u8; 16];

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

pub(crate) struct Hex<'a, const N: usize>(pub(crate) &'a [u8; N]);

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

const HEX: &[u8; 16] = b"0123456789abcdef";

impl Display for Hex<'_, 16> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut out = [0u8; 32];
        for (i, byte) in self.0.iter().enumerate() {
            out[i * 2] = HEX[(byte >> 4) as usize];
            out[i * 2 + 1] = HEX[(byte & 0x0f) as usize];
        }

        write!(f, "{}", unsafe { std::str::from_utf8_unchecked(&out) }) // Safety: HEX is all valid UTF-8
    }
}

impl Display for Hex<'_, 32> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut out = [0u8; 64];
        for (i, byte) in self.0.iter().enumerate() {
            out[i * 2] = HEX[(byte >> 4) as usize];
            out[i * 2 + 1] = HEX[(byte & 0x0f) as usize];
        }

        write!(f, "{}", unsafe { std::str::from_utf8_unchecked(&out) }) // Safety: HEX is all valid UTF-8
    }
}
