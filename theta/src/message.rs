use std::{any::Any, fmt::Debug, sync::Arc};

use futures::channel::oneshot;

use serde::{Deserialize, Serialize};
use theta_flume::{Receiver, Sender, WeakSender};
use tokio::sync::Notify;

use crate::context::Context;
#[cfg(feature = "remote")]
use crate::{actor::Actor, actor_ref::ActorHdl, monitor::AnyReportTx};

pub type MsgPack<A: Actor> = (A::Msg, Continuation);

pub type OneShot = oneshot::Sender<Box<dyn Any + Send>>;

pub type MsgTx<A> = Sender<MsgPack<A>>;
pub type WeakMsgTx<A> = WeakSender<MsgPack<A>>;
pub type MsgRx<A> = Receiver<MsgPack<A>>;

pub type SigTx = Sender<RawSignal>;
pub type WeakSigTx = WeakSender<RawSignal>;
pub type SigRx = Receiver<RawSignal>;

pub trait Message<A: Actor>:
    Debug + Send + Into<A::Msg> + Serialize + for<'de> Deserialize<'de> + 'static
{
    type Return: Debug + Send + Sync + Serialize + for<'de> Deserialize<'de> + 'static;

    fn process(
        state: &mut A,
        ctx: Context<A>,
        msg: Self,
    ) -> impl Future<Output = Self::Return> + Send;

    // async fn process_to_any(state: &mut A, ctx: Context<A>, msg: Self) -> Box<dyn Any + Send> {
    fn process_to_any(
        state: &mut A,
        ctx: Context<A>,
        msg: Self,
    ) -> impl Future<Output = Box<dyn Any + Send>> + Send {
        async move { Box::new(Self::process(state, ctx, msg).await) as Box<dyn Any + Send> }
    }

    // async fn process_to_bytes(state: &mut A, ctx: Context<A>, msg: Self) -> Vec<u8> {
    fn process_to_bytes(
        state: &mut A,
        ctx: Context<A>,
        msg: Self,
    ) -> impl Future<Output = Vec<u8>> + Send {
        async move {
            let ret = Self::process(state, ctx, msg).await;
            postcard::to_stdvec(&ret).unwrap()
        }
    }
}

/// A continuation is another actor, which is regular actor or reply channel.
/// Per specification, address does not need to tell the identity of the actor,
/// Which means this is kind of ad-hoc address of continuation actor.
#[derive(Debug)]
pub enum Continuation {
    Reply(Option<OneShot>),
    Forward(OneShot),
}

// ? Is poison pill even necessary?

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

    pub fn send(self, res: Box<dyn Any + Send>) -> Result<(), Box<dyn Any + Send>> {
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
