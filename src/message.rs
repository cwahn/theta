use std::any::Any;

use futures::{FutureExt, future::BoxFuture};
use tokio::sync::oneshot;

use crate::{actor::Actor, base::Immutable, context::Context};

/// A continuation is another actor, which is regular actor or reply channel.
/// Per specification, address does not need to tell the identity of the actor,
/// Which means this is kind of ad-hoc address of continuation actor.
pub type Continuation = oneshot::Sender<Box<dyn Any + Send>>;

pub type DynMessage<A> = Box<dyn Message<A>>;

// ? Is poison pill even necessary?

pub trait Behavior<T: Send + 'static>: Actor {
    type Return: Send + 'static;

    const IS_POISON_PILL: bool = false;

    fn handle(&mut self, ctx: &Context<Self>, msg: T) -> impl Future<Output = Self::Return> + Send;
}

pub trait Message<A>: Send
where
    A: Actor,
{
    fn handle_dyn<'a>(
        self: Box<Self>,
        actor: Immutable<'a, A>,
        ctx: &'a Context<A>,
    ) -> BoxFuture<'a, Box<dyn Any + Send>>;

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
    ) -> BoxFuture<'a, Box<dyn Any + Send>> {
        async move {
            let result = A::handle(actor.0, ctx, *self).await;
            Box::new(result) as Box<dyn Any + Send>
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

    fn handle(&mut self, ctx: &Context<Self>, _msg: PoisonPill) -> BoxFuture<'_, Self::Return> {
        Box::pin(async move {
            // If it already get terminate, or ignore signal, it might fail but doesn't matter
            // let _ = ctx.self_ref.signal_tx.send(RawSignal::Terminate(None));
            // ? How to send signal to self?
            todo!()
        })
    }
}
