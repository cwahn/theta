use std::fmt::Debug;

use crate::{
    actor_ref::{ActorRef, SupervisionRef, WeakActorRef},
    actor_status::ActorStatus,
    base::Immutable,
    context::{Context, SupervisionContext},
    continuation::Continuation,
    message::DynMessage,
    signal::{Escalation, RawSignal, Signal},
};

pub trait Actor: Sized + Debug + Send + 'static {
    type Args: Send + 'static;
    type Error: std::error::Error + Send + Sync + 'static;

    /// An initialization logic of an actor.
    /// - Panic-safe; panic will get caught and escalated
    fn on_start(args: Self::Args, ctx: &Context<Self>) -> impl Future<Output = Self> + Send; // Should start or panic

    /// A wrapper around message handling
    /// - Panic-safe; panic will get caught and escalated
    /// - Error will not imediately escalate, but handled by `on_error`
    #[inline]
    #[allow(unused_variables)]
    fn on_message(
        // &mut self,
        state: Immutable<Self>,
        ctx: &Context<Self>,
        msg: DynMessage<Self>,
        k: Option<Continuation>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async move {
            let result = msg.handle_dyn(state, ctx).await?;
            if let Some(k) = k {
                let _ = k.send(result);
            }
            Ok(())
        }
    }

    /// A chance to recover error of self before escalation with access to the supervision context
    /// - Will get called on error, but not on panic or abortion
    /// - Should return Ok(()) to recover or Err(err) to escalate
    /// - Panic-safe; panic will get caught and escalated
    #[inline]
    #[allow(unused_variables)]
    fn on_error(
        &mut self,
        ctx: &SupervisionContext<Self>,
        err: Self::Error,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        Box::pin(async move {
            Err(err) // For now, just return the error
        })
    }

    /// Handles escalation from children
    /// - Panic-safe; panic will get caught and escalated
    #[inline]
    #[allow(unused_variables)]
    fn on_escalation(
        &mut self,
        ctx: &SupervisionContext<Self>,
        // ! Can't get actor_ref of any subordinate, because it is not available in the context
        supervision_ref: SupervisionRef,
        escalation: Escalation,
    ) -> impl Future<Output = Result<Signal, Self::Error>> + Send {
        async move { todo!() }
    }

    /// A cleanup logic of an actor, right before stop of event loop
    /// - ! NOT panic-safe; panic will likely cause failure of the actor system
    #[inline]
    #[allow(unused_variables)]
    fn on_stop(
        &mut self,
        ctx: &SupervisionContext<Self>,
        status: ActorStatus<Self>,
    ) -> impl Future<Output = ()> + Send {
        async { () }
    }
}
