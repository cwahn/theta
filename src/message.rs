use std::{any::Any, sync::Arc};

use futures::{FutureExt, future::BoxFuture};
use tokio::sync::{Notify, oneshot};

use crate::{actor::Actor, actor_ref::ActorHdl, context::Context};

/// A continuation is another actor, which is regular actor or reply channel.
/// Per specification, address does not need to tell the identity of the actor,
/// Which means this is kind of ad-hoc address of continuation actor.
pub type Continuation = oneshot::Sender<Box<dyn Any + Send>>;

pub type DynMessage<A> = Box<dyn Message<A>>;

// ? Is poison pill even necessary?

pub trait Behavior<M: Send + 'static>: Actor {
    type Return: Send + 'static;

    fn process(&mut self, ctx: Context<Self>, msg: M) -> impl Future<Output = Self::Return> + Send;
}

pub trait Message<A>: Send
where
    A: Actor,
{
    fn process_dyn<'a>(
        self: Box<Self>,
        ctx: Context<'a, A>,
        state: &'a mut A,
    ) -> BoxFuture<'a, Box<dyn Any + Send>>;
}

pub struct PoisonPill {}

#[derive(Debug, Clone, Copy)]
pub enum Signal {
    Pause,
    Resume,
    /// Restart the actor
    Restart,
    Terminate,
}

#[derive(Debug, Clone)]
pub enum Escalation {
    Error(String),
    PanicOnInit(String),
    PanicOnMessage(String),
    PanicOnSupervision(String),
}

#[derive(Debug)]
pub enum RawSignal {
    Escalation(ActorHdl, Escalation),
    ChildDropped,

    Pause(Option<Arc<Notify>>),
    Resume(Option<Arc<Notify>>),
    // If actor's initialization failed, it will notifed as complete then will get escalation.
    Restart(Option<Arc<Notify>>),
    Terminate(Option<Arc<Notify>>),
}

// Implementations

impl<A, M> Message<A> for M
where
    A: Actor + Behavior<M>,
    M: Send + 'static,
{
    fn process_dyn<'a>(
        self: Box<Self>,
        ctx: Context<'a, A>,
        state: &'a mut A,
    ) -> BoxFuture<'a, Box<dyn Any + Send>> {
        async move {
            let res = state.process(ctx, *self).await;
            Box::new(res) as Box<dyn Any + Send>
        }
        .boxed()
    }
}
