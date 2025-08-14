use std::{fmt::Debug, future::Future, panic::UnwindSafe};

use crate::{
    context::Context,
    message::{Continuation, Escalation, Signal},
};

#[cfg(feature = "remote")]
use {
    crate::remote::{base::ActorTypeId, serde::FromTaggedBytes},
    serde::{Deserialize, Serialize},
};

pub type ActorId = uuid::Uuid;

// ! Currently, the restart sementic should be considered ill-designed.
// todo Redesign resilience system.

pub trait ActorArgs: Clone + Send + UnwindSafe + 'static {
    type Actor: Actor;

    /// An initialization logic of an actor.
    /// - Panic-safe; panic will get caught and escalated
    fn initialize(
        ctx: Context<Self::Actor>,
        args: &Self,
    ) -> impl Future<Output = Self::Actor> + Send + UnwindSafe;
}

pub trait Actor: Sized + Debug + Send + UnwindSafe + 'static {
    #[cfg(not(feature = "remote"))]
    type Msg: Send;
    #[cfg(feature = "remote")]
    type Msg: Send + Serialize + for<'de> Deserialize<'de> + FromTaggedBytes;

    /// A type used for monitoring the actor state.
    #[cfg(not(feature = "remote"))]
    type StateReport: Send + UnwindSafe + Clone + for<'a> From<&'a Self> + 'static;

    #[cfg(feature = "remote")]
    type StateReport: Send
        + UnwindSafe
        + Clone
        + for<'a> From<&'a Self>
        + Serialize
        + for<'de> Deserialize<'de>;

    /// A wrapper around message processing for optional monitoring.
    /// - Panic-safe; panic will get caught and escalated
    /// - Could be manually implemented, but recommended to use ['intention!'] macro
    #[allow(unused_variables)]
    fn process_msg(
        &mut self,
        ctx: Context<Self>,
        msg: Self::Msg,
        k: Continuation,
    ) -> impl Future<Output = ()> + Send;

    /// Handles escalation from children
    /// - Panic-safe; panic will get caught and escalated
    /// - It is recommended to set max_restart and in_period to prevent infinite loop
    #[allow(unused_variables)]
    fn supervise(
        &mut self,
        escalation: Escalation,
    ) -> impl Future<Output = (Signal, Option<Signal>)> + Send {
        __default_supervise(self, escalation)
    }

    /// Called on on restart, before initialization
    /// - Panic-safe; but the panic will not be escalated but ignored or logged
    /// - State might be corrupted since it does not rollback on panic
    #[allow(unused_variables)]
    fn on_restart(&mut self) -> impl Future<Output = ()> + Send {
        __default_on_restart(self)
    }

    /// Called on drop, or termination
    /// - Panic-safe; but the panic will not be escalated but ignored or logged
    /// - In case of termination, state might be corrupted since it does not rollback on panic
    /// - Since the message loop is already stopped, any message to self will be lost
    #[allow(unused_variables)]
    fn on_exit(&mut self, exit_code: ExitCode) -> impl Future<Output = ()> + Send {
        __default_on_exit(self, exit_code)
    }

    #[allow(unused_variables)]
    fn hash_code(&self) -> u64 {
        0 // no-op by default
    }

    #[allow(unused_variables)]
    fn state_report(&self) -> Self::StateReport {
        self.into() // no-op by default
    }

    /// Should not implemented by user.
    #[cfg(feature = "remote")]
    const IMPL_ID: ActorTypeId;

    // #[cfg(feature = "remote")]
}

#[derive(Debug, Clone)]
pub enum ExitCode {
    Dropped,
    Terminated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nil;

// Delegated default implementation in order to decouple it from macro expansion

pub async fn __default_supervise<A: Actor>(
    _actor: &mut A,
    _escalation: Escalation,
) -> (Signal, Option<Signal>) { (Signal::Terminate, None) }

pub async fn __default_on_restart<A: Actor>(
    _actor: &mut A,
) {}

pub async fn __default_on_exit<A: Actor>(
    _actor: &mut A,
    _exit_code: ExitCode,
) {}

// Implementations

impl<T> From<&T> for Nil
where
    T: Actor,
{
    fn from(_: &T) -> Self {
        Nil
    }
}
