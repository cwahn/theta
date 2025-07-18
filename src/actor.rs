use std::fmt::Debug;

use crate::{
    actor_instance::ExitCode,
    actor_ref::SupervisionRef,
    base::Immutable,
    context::{Context, SupervisionContext},
    message::{Continuation, DynMessage, Escalation, Signal},
};

pub trait Actor: Sized + Debug + Send + 'static {
    type Args: Send + 'static;

    /// An initialization logic of an actor.
    /// - Panic-safe; panic will get caught and escalated
    fn init(ctx: Context<Self>, args: &Self::Args) -> impl Future<Output = Self> + Send; // Should start or panic

    /// A wrapper around message handling
    /// - Panic-safe; panic will get caught and escalated
    #[inline]
    #[allow(unused_variables)]
    fn on_message(
        ro_self: Immutable<Self>,
        ctx: Context<Self>,
        msg: DynMessage<Self>,
        k: Option<Continuation>,
    ) -> impl Future<Output = ()> + Send {
        async move {
            let result = msg.handle_dyn(ro_self, ctx).await;
            if let Some(k) = k {
                let _ = k.send(result);
            }
        }
    }

    /// Handles escalation from children
    /// - Panic-safe; panic will get caught and escalated
    #[inline]
    #[allow(unused_variables)]
    fn supervise(
        &mut self,
        ctx: SupervisionContext<Self>,
        child_ref: SupervisionRef,
        escalation: Escalation,
    ) -> impl Future<Output = ()> + Send {
        async move {
            let _ = child_ref.signal(Signal::Restart).await;
        }
    }

    /// Called on on restart, before initialization
    /// - Panic-safe; but the panic will not be escalated but ignored or logged
    /// - State might be corrupted since it does not rollback on panic
    #[inline]
    #[allow(unused_variables)]
    fn on_restart(&mut self, ctx: Context<Self>) -> impl Future<Output = ()> + Send {
        async move { () }
    }

    /// Called on drop, or termination
    /// - Panic-safe; but the panic will not be escalated but ignored or logged
    /// - In case of termination, state might be corrupted since it does not rollback on panic
    /// - Since the message loop is already stopped, any message to self will be lost
    #[inline]
    #[allow(unused_variables)]
    fn on_exit(
        &mut self,
        ctx: Context<Self>,
        exit_code: ExitCode,
    ) -> impl Future<Output = ()> + Send {
        async { () }
    }
}
