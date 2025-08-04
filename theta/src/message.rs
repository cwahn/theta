use std::{any::Any, fmt::Debug, sync::Arc};

use futures::{FutureExt, channel::oneshot, future::BoxFuture};
use serde::{Deserialize, Serialize};
use tokio::sync::{
    Notify,
    mpsc::{UnboundedReceiver, UnboundedSender, WeakUnboundedSender},
};

// #[cfg(feature = "remote")]
// use crate::remote::serde::BehaviorImplId;
use crate::{
    actor::Actor,
    actor_ref::ActorHdl,
    context::Context,
    monitor::AnyReportTx,
    remote::{MsgTid, Remote},
};

pub type BoxedMsg<A> = Box<dyn Message<A>>;
pub type MsgPack<A> = (BoxedMsg<A>, Continuation);

pub type OneShot = oneshot::Sender<Box<dyn Any + Send + Sync>>;

pub type MsgTx<A> = UnboundedSender<MsgPack<A>>;
pub type WeakMsgTx<A> = WeakUnboundedSender<MsgPack<A>>;
pub type MsgRx<A> = UnboundedReceiver<MsgPack<A>>;

pub type SigTx = UnboundedSender<RawSignal>;
pub type WeakSigTx = WeakUnboundedSender<RawSignal>;
pub type SigRx = UnboundedReceiver<RawSignal>;

/// A continuation is another actor, which is regular actor or reply channel.
/// Per specification, address does not need to tell the identity of the actor,
/// Which means this is kind of ad-hoc address of continuation actor.
#[derive(Debug)]
pub enum Continuation {
    Reply(Option<OneShot>),
    Forward(OneShot),
}

// ? Is poison pill even necessary?

pub trait Behavior<M: Send + 'static>: Actor {
    type Return: Debug + Send + Sync + 'static;

    fn process(&mut self, ctx: Context<Self>, msg: M) -> impl Future<Output = Self::Return> + Send;
}

pub trait Message<A>: Debug + Send + Sync + erased_serde::Serialize
where
    A: Actor,
{
    fn process_dyn<'a>(
        self: Box<Self>,
        ctx: Context<A>,
        state: &'a mut A,
    ) -> BoxFuture<'a, Box<dyn Any + Send + Sync>>;
}

// Implementations

impl Continuation {
    pub const fn nil() -> Self {
        Continuation::Reply(None)
    }

    pub fn reply(tx: OneShot) -> Self {
        Continuation::Reply(Some(tx))
    }

    pub fn forward(tx: OneShot) -> Self {
        Continuation::Forward(tx)
    }

    pub fn send(self, res: Box<dyn Any + Send + Sync>) -> Result<(), Box<dyn Any + Send + Sync>> {
        match self {
            Continuation::Reply(mb_tx) => match mb_tx {
                Some(tx) => tx.send(res),
                None => Ok(()),
            },
            Continuation::Forward(tx) => tx.send(res),
        }
    }

    pub fn is_nil(&self) -> bool {
        matches!(self, Continuation::Reply(None))
    }
}

impl<A, M> Message<A> for M
where
    A: Actor + Behavior<M>,
    M: Debug + Send + Sync + 'static,
{
    fn process_dyn<'a>(
        self: Box<Self>,
        ctx: Context<A>,
        state: &'a mut A,
    ) -> BoxFuture<'a, Box<dyn Any + Send + Sync>> {
        async move {
            let res = state.process(ctx, *self).await;
            Box::new(res) as Box<dyn Any + Send + Sync>
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
    Observe(AnyReportTx),

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
