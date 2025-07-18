use std::fmt::Debug;

use crate::{
    actor_ref::SupervisionRef,
    base::Immutable,
    context::{Context, SupervisionContext},
    message::{Continuation, DynMessage},
    signal::{Escalation, Signal},
};

pub trait Actor: Sized + Debug + Send + 'static {
    type Args: Clone + Send + 'static;
    // type Error: std::error::Error + Send + Sync + 'static;

    /// An initialization logic of an actor.
    /// - Panic-safe; panic will get caught and escalated
    fn init(ctx: Context<Self>, args: &Self::Args) -> impl Future<Output = Self> + Send; // Should start or panic

    /// A wrapper around message handling
    /// - Panic-safe; panic will get caught and escalated
    /// - Error will not imediately escalate, but handled by `on_error`
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

    // /// A chance to recover error of self before escalation with access to the supervision context
    // /// - Will get called on error, but not on panic or abortion
    // /// - Should return Ok(()) to recover or Err(err) to escalate
    // /// - Panic-safe; panic will get caught and escalated
    // #[inline]
    // #[allow(unused_variables)]
    // fn on_error(
    //     &mut self,
    //     ctx: &SupervisionContext<Self>,
    //     err: Self::Error,
    // ) -> impl Future<Output = Result<(), Self::Error>> + Send {
    //     Box::pin(async move {
    //         Err(err) // For now, just return the error
    //     })
    // }

    /// Handles escalation from children
    /// - Panic-safe; panic will get caught and escalated
    #[inline]
    #[allow(unused_variables)]
    fn on_escalation(
        &mut self,
        ctx: SupervisionContext<Self>,
        supervision_ref: SupervisionRef,
        escalation: Escalation,
    ) -> impl Future<Output = ()> + Send {
        async move {
            supervision_ref.signal(Signal::Restart).await;
        }
    }

    // ? Do I need this?
    // /// A chance to monitor the panic
    // #[inline]
    // #[allow(unused_variables)]
    // fn on_panic(ro_self: Immutable<Self>) -> impl Future<Output = ()> + Send {
    //     async move { () }
    // }

    /// A cleanup logic of an actor, right before stop of event loop
    /// - Panic-safe; but
    #[inline]
    #[allow(unused_variables)]
    fn on_stop(
        &mut self,
        ctx: Context<Self>,
        status: ActorStatus<Self>,
    ) -> impl Future<Output = ()> + Send {
        async { () }
    }
}
