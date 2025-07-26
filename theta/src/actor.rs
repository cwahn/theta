use std::{fmt::Debug, panic::UnwindSafe};

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
    /// A wrapper around message processing for optional monitoring.
    /// - Panic-safe; panic will get caught and escalated
    #[allow(unused_variables)]
    fn process_msg(
        &mut self,
        ctx: Context<Self>,
        msg: DynMessage<Self>,
        k: Continuation,
    ) -> impl Future<Output = ()> + Send {
        async move {
            let res = msg.process_dyn(ctx, self).await;
            let _ = k.send(res);
        }
    }

    /// Handles escalation from children
    /// - Panic-safe; panic will get caught and escalated
    /// - It is recommended to set max_restart and in_period to prevent infinite loop
    #[allow(unused_variables)]
    fn supervise(
        &mut self,
        escalation: Escalation,
    ) -> impl Future<Output = (Signal, Option<Signal>)> + Send {
        async move { (Signal::Terminate, None) }
    }

    /// Called on on restart, before initialization
    /// - Panic-safe; but the panic will not be escalated but ignored or logged
    /// - State might be corrupted since it does not rollback on panic
    #[allow(unused_variables)]
    fn on_restart(&mut self) -> impl Future<Output = ()> + Send {
        async move {}
    }

    /// Called on drop, or termination
    /// - Panic-safe; but the panic will not be escalated but ignored or logged
    /// - In case of termination, state might be corrupted since it does not rollback on panic
    /// - Since the message loop is already stopped, any message to self will be lost
    #[allow(unused_variables)]
    fn on_exit(&mut self, exit_code: ExitCode) -> impl Future<Output = ()> + Send {
        async {}
    }

    /// Should not implemented by user.
    // fn __impl_id(&self) -> ImplId;
    #[cfg(feature = "remote")]
    const __IMPL_ID: ImplId;
}
