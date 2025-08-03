use std::{
    fmt::Debug,
    future::Future,
    hash::{Hash, Hasher},
    panic::UnwindSafe,
};

use rustc_hash::FxHasher;
use serde::{Deserialize, Serialize};

use crate::{
    base::ImplId,
    context::Context,
    error::ExitCode,
    message::{Continuation, DynMessage, Escalation, Signal},
};

pub type ActorId = uuid::Uuid;

// ! Currently, the restart sementic should be considered ill-designed.
// todo Redesign resilience system.

pub trait ActorConfig: Clone + Send + UnwindSafe + 'static {
    type Actor: Actor;

    /// An initialization logic of an actor.
    /// - Panic-safe; panic will get caught and escalated
    fn initialize(
        ctx: Context<Self::Actor>,
        cfg: &Self,
    ) -> impl Future<Output = Self::Actor> + Send + UnwindSafe;
}

pub trait Actor: Sized + Debug + Send + UnwindSafe + 'static {
    /// A type used for monitoring the actor state.
    type StateReport: for<'a> From<&'a Self>
        + Clone
        + Serialize
        + for<'d> Deserialize<'d>
        + Send
        + Sync;

    /// A wrapper around message processing for optional monitoring.
    /// - Panic-safe; panic will get caught and escalated
    #[allow(unused_variables)]
    fn process_msg(
        &mut self,
        ctx: Context<Self>,
        msg: DynMessage<Self>,
        k: Continuation,
    ) -> impl Future<Output = ()> + Send {
        __default_process_msg(self, ctx, msg, k)
    }

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
    const __IMPL_ID: ImplId;
}

// pub trait ObservableActor: Actor + Hash {
//     type StateReport: for<'a> From<&'a Self>
//         + Clone
//         + Serialize
//         + for<'d> Deserialize<'d>
//         + Send
//         + Sync;

//     fn hash_code(&self) -> u64 {
//         let mut hasher = FxHasher::default();
//         self.hash(&mut hasher);
//         hasher.finish()
//     }

//     fn state_report(&self) -> Self::StateReport {
//         self.into()
//     }
// }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nil;

// Delegated default implementation in order to decouple it from macro expansion

pub fn __default_process_msg<A: Actor>(
    actor: &mut A,
    ctx: Context<A>,
    msg: DynMessage<A>,
    k: Continuation,
) -> impl std::future::Future<Output = ()> + Send {
    async move {
        let res = msg.process_dyn(ctx, actor).await;
        let _ = k.send(res);
    }
}

pub fn __default_supervise<A: Actor>(
    _actor: &mut A,
    _escalation: Escalation,
) -> impl std::future::Future<Output = (Signal, Option<Signal>)> + Send {
    async move { (Signal::Terminate, None) }
}

pub fn __default_on_restart<A: Actor>(
    _actor: &mut A,
) -> impl std::future::Future<Output = ()> + Send {
    async move {}
}

pub fn __default_on_exit<A: Actor>(
    _actor: &mut A,
    _exit_code: ExitCode,
) -> impl std::future::Future<Output = ()> + Send {
    async move {}
}

// Implementations

impl<T> From<&T> for Nil
where
    T: Actor,
{
    fn from(_: &T) -> Self {
        Nil
    }
}
