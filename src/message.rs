use std::{any::Any, sync::Arc};

use futures::{FutureExt, future::BoxFuture};
use tokio::sync::Notify;

use crate::{
    actor::{self, Actor},
    base::Immutable,
    context::Context,
    continuation::Continuation,
    signal::RawSignal,
};

pub type DynMessage<A> = Box<dyn Message<A>>;

pub trait Behavior<T: Send + 'static>: Actor {
    type Return: Send + 'static;

    const IS_POISON_PILL: bool = false;

    fn handle(
        &mut self,
        ctx: &Context<Self>,
        msg: T,
    ) -> impl Future<Output = Result<Self::Return, Self::Error>> + Send;
}

pub trait Message<A>: Send
where
    A: Actor,
{
    fn handle_dyn<'a>(
        self: Box<Self>,
        // state: Immutable<A>,
        // actor: &'a mut A,
        actor: Immutable<'a, A>,
        ctx: &'a Context<A>,
        // k: Option<Continuation>,
    ) -> BoxFuture<'a, Result<Box<dyn Any + Send>, A::Error>>;

    fn is_poison_pill(&self) -> bool {
        A::IS_POISON_PILL
    }
}

pub struct PoisonPill {}

// Implementations

impl<A, M> Message<A> for M
where
    A: Actor + Behavior<M>,
    M: Send + 'static,
{
    fn handle_dyn<'a>(
        self: Box<Self>,
        actor: Immutable<'a, A>,
        ctx: &'a Context<A>,
        // k: Option<Continuation>,
    ) -> BoxFuture<'a, Result<Box<dyn Any + Send>, A::Error>> {
        async move {
            let result = A::handle(actor.0, ctx, *self).await;
            match result {
                Ok(res) => Ok(Box::new(res) as Box<dyn Any + Send>),
                Err(err) => Err(err),
            }
        }
        .boxed()
    }
}

impl<A> Behavior<PoisonPill> for A
where
    A: Actor,
{
    type Return = ();

    const IS_POISON_PILL: bool = true;

    fn handle(
        &mut self,
        ctx: &Context<Self>,
        _msg: PoisonPill,
    ) -> impl Future<Output = Result<Self::Return, Self::Error>> + Send {
        async move {
            // If it already get terminate, or ignore signal, it might fail but doesn't matter
            let _ = ctx.self_ref.signal_tx.send(RawSignal::Terminate(None));

            Ok(())
        }
    }
}
