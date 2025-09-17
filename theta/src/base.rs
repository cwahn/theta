//! Base types and utilities used throughout the framework.

use serde::{Deserialize, Serialize};
use std::{
    any::{Any, type_name},
    fmt::{Debug, Display, Formatter},
    marker::PhantomData,
};
use thiserror::Error;

use crate::{
    actor::{Actor, ActorId},
    context::LookupError,
};

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

pub(crate) struct DebugActorId<A: Actor>(ActorId, PhantomData<A>);

pub(crate) struct DebugActorIdent<'a, A: Actor>(&'a Ident, PhantomData<A>);

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

impl<A: Actor> DebugActorId<A> {
    pub(crate) fn new(id: ActorId) -> Self {
        Self(id, PhantomData)
    }
}

impl<A: Actor> Display for DebugActorId<A> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", type_name::<A>(), self.0)
    }
}

impl<'a, A: Actor> DebugActorIdent<'a, A> {
    pub(crate) fn new(ident: &'a Ident) -> Self {
        Self(ident, PhantomData)
    }
}

impl<'a, A: Actor> Display for DebugActorIdent<'a, A> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({:02x?})", type_name::<A>(), self.0)
    }
}
