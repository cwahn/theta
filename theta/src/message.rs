use std::{any::Any, fmt::Debug, sync::Arc};

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

    fn process(
        &mut self,
        ctx: Context<'_, Self>,
        msg: M,
    ) -> impl Future<Output = Self::Return> + Send;
}

pub trait Message<A>: Debug + Send
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
    Restart,
    Terminate,
}

#[derive(Debug, Clone)]
pub enum Escalation {
    Initialize(String),
    ProcessMsg(String),
    Supervise(String),
}

#[derive(Debug)]
pub enum RawSignal {
    Escalation(ActorHdl, Escalation),
    ChildDropped,

    Pause(Option<Arc<Notify>>),
    Resume(Option<Arc<Notify>>),
    Restart(Option<Arc<Notify>>),
    Terminate(Option<Arc<Notify>>),
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum InternalSignal {
    Pause,
    Resume,
    Restart,
    Terminate,
}

// Implementations

impl<A, M> Message<A> for M
where
    A: Actor + Behavior<M>,
    M: Debug + Send + 'static,
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

impl From<Signal> for InternalSignal {
    fn from(signal: Signal) -> Self {
        match signal {
            Signal::Restart => InternalSignal::Restart,
            Signal::Terminate => InternalSignal::Terminate,
        }
    }
}
