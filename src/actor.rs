use core::panic;
use std::fmt::Debug;

use crate::{
    actor_instance::ExitCode,
    actor_ref::ActorHdl,
    context::Context,
    message::{Continuation, DynMessage, Escalation, Signal},
};

pub trait Actor: Sized + Debug + Send + 'static {
    type Args: Send + 'static;

    /// An initialization logic of an actor.
    /// - Panic-safe; panic will get caught and escalated
    fn initialize(ctx: Context<'_, Self>, args: &Self::Args) -> impl Future<Output = Self> + Send; // Should start or panic

    /// A wrapper around message processing for optional monitoring.
    /// - Panic-safe; panic will get caught and escalated
    #[allow(unused_variables)]
    fn process_msg(
        &mut self,
        ctx: Context<'_, Self>,
        msg: DynMessage<Self>,
        k: Option<Continuation>,
    ) -> impl Future<Output = ()> + Send {
        async move {
            let res = msg.process_dyn(ctx, self).await;
            k.map(|k| k.send(res));
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
        async move { () }
    }

    /// Called on drop, or termination
    /// - Panic-safe; but the panic will not be escalated but ignored or logged
    /// - In case of termination, state might be corrupted since it does not rollback on panic
    /// - Since the message loop is already stopped, any message to self will be lost
    #[allow(unused_variables)]
    fn on_exit(&mut self, exit_code: ExitCode) -> impl Future<Output = ()> + Send {
        async { () }
    }
}
