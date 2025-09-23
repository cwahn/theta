//! Base types and utilities used throughout the framework.

use serde::{Deserialize, Serialize};
use std::{
    any::Any,
    fmt::{Debug, Display},
};
use thiserror::Error;
use uuid::Uuid;

use crate::actor::Actor;

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

/// Errors that can occur during operations over bindings.
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum BindingError {
    #[error("invalid identifier")]
    InvalidIdent,
    #[error("actor not found")]
    NotFound,
    #[error("actor type id mismatch")]
    TypeMismatch,
    #[error("actor ref downcast failed")]
    DowncastError,
    #[cfg(feature = "remote")]
    #[error(transparent)]
    SerializeError(#[from] postcard::Error),
}

/// Errors that can occur when monitoring actor status.
/// This is here in order to keep the inter-peer type does not depend on feature flags.
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum MonitorError {
    #[error(transparent)]
    BindingError(#[from] BindingError),
    #[error("failed to send signal")]
    SigSendError,
}

pub(crate) struct Hex<'a, const N: usize>(pub(crate) &'a [u8; N]);

/// Parse string identifier into 16-byte identifier.
/// Since all ASCII &str shorter than 16 bytes is not a valid UUID as byte[8] should be 0x80–0xBF which is non ASCII,
/// Further for unicode, unless one types in \u{00A0}, continuation on byte[8] it could not be a valid UUID either.
/// Both canonical hyphenated UUID string and all 32 hex digits never fits in 16 bytes.
/// Therefore, it does not accidentally interpret a human-readable name as UUID.
pub(crate) fn parse_ident(s: &str) -> Result<Ident, BindingError> {
    match Uuid::try_parse(s) {
        Err(_) => {
            let bytes = s.as_bytes();

            if bytes.len() > 16 {
                return Err(BindingError::InvalidIdent);
            }

            let mut ident = [0u8; 16];
            ident[..bytes.len()].copy_from_slice(bytes);

            Ok(ident)
        }
        Ok(uuid) => Ok(*uuid.as_bytes()),
    }
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
