use std::{any::Any, fmt::Debug, sync::Arc};

use futures::channel::oneshot;

#[cfg(feature = "remote")]
use serde::{Deserialize, Serialize};
use theta_flume::{Receiver, Sender, WeakSender};
use tokio::sync::Notify;

#[cfg(feature = "remote")]
use crate::remote::base::Tag;
use crate::{actor::Actor, actor_ref::ActorHdl, context::Context, monitor::AnyReportTx};

pub type MsgPack<A: Actor> = (A::Msg, Continuation);

pub type OneShot = oneshot::Sender<Box<dyn Any + Send>>;

pub type MsgTx<A> = Sender<MsgPack<A>>;
pub type WeakMsgTx<A> = WeakSender<MsgPack<A>>;
pub type MsgRx<A> = Receiver<MsgPack<A>>;

pub type SigTx = Sender<RawSignal>;
pub type WeakSigTx = WeakSender<RawSignal>;
pub type SigRx = Receiver<RawSignal>;

pub trait Message<A: Actor>: Debug + Send + Into<A::Msg> + 'static {
    #[cfg(not(feature = "remote"))]
    type Return: Send + UnwindSafe + 'static;

    #[cfg(feature = "remote")]
    type Return: Debug + Send + Sync + Serialize + for<'de> Deserialize<'de> + 'static;

    #[cfg(feature = "remote")]
    const TAG: Tag;

    fn process(
        state: &mut A,
        ctx: Context<A>,
        msg: Self,
    ) -> impl Future<Output = Self::Return> + Send;

    fn process_to_any(
        state: &mut A,
        ctx: Context<A>,
        msg: Self,
    ) -> impl Future<Output = Box<dyn Any + Send>> + Send {
        async move { Box::new(Self::process(state, ctx, msg).await) as Box<dyn Any + Send> }
    }

    #[cfg(feature = "remote")]
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

    // ? Anyway I can call this?
    #[cfg(feature = "remote")]
    fn process_to_forward<B>(
        state: &mut A,
        ctx: Context<A>,
        msg: Self,
    ) -> impl Future<Output = B::Msg> + Send
    where
        B: Actor,
        Self::Return: Message<B>,
    {
        async move { Self::process(state, ctx, msg).await.into() }
    }
}

/// A continuation is another actor, which is regular actor or reply channel.
/// Per specification, address does not need to tell the identity of the actor,
/// Which means this is kind of ad-hoc address of continuation actor.
#[derive(Debug)]
pub enum Continuation {
    Nil,

    Reply(OneShot),   // type erased return
    Forward(OneShot), // type erased return

    RemoteReply(oneshot::Sender<Vec<u8>>), // Serialized return
                                           // ! For the other cases, the recepient knows the type, but for the forwarding no one knows
                                           // RemoteForward(OneShot), // ? How to designate what is the msg type.
}

#[derive(Debug, Clone, Copy)]
pub enum Signal {
    /// Restart the actor terminating all the descendants
    Restart,
    /// Terminate the actor with all the descendants
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

// Implementations

impl Continuation {
    pub fn reply(tx: OneShot) -> Self {
        Continuation::Reply(tx)
    }

    pub fn forward(tx: OneShot) -> Self {
        Continuation::Forward(tx)
    }

    // pub fn send(self, res: Box<dyn Any + Send>) -> Result<(), Box<dyn Any + Send>> {
    //     match self {
    //         Continuation::Reply(tx) => tx.send(res),
    //         Continuation::Forward(tx) => tx.send(res),

    //     }
    // }

    pub fn is_nil(&self) -> bool {
        matches!(self, Continuation::Nil)
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
