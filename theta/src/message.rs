use std::{any::Any, fmt::Debug, sync::Arc};

use futures::channel::oneshot;

#[cfg(feature = "remote")]
use serde::{Deserialize, Serialize};
use theta_flume::{Receiver, Sender, WeakSender};
use tokio::sync::Notify;

#[cfg(feature = "remote")]
use crate::remote::base::Tag;
use crate::{
    actor::Actor, actor_ref::ActorHdl, context::Context, monitor::AnyReportTx,
    remote::peer::RemotePeer,
};

pub type MsgPack<A: Actor> = (A::Msg, Continuation);

// ? Can I consider oneshot Sender as unwind safe?
pub type OneShotAny = oneshot::Sender<Box<dyn Any + Send>>;
pub type OneShotBytes = oneshot::Sender<Vec<u8>>;

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
        peer: Arc<RemotePeer>,
        msg: Self,
    ) -> impl Future<Output = Vec<u8>> + Send {
        async move {
            use crate::remote::peer::CURRENT_PEER;

            let ret = Self::process(state, ctx, msg).await;

            CURRENT_PEER.sync_scope(peer, || {
                // ! todo Handle serialization error
                postcard::to_stdvec(&ret).unwrap()
            })
        }
    }
}

/// A continuation is another actor, which is regular actor or reply channel.
/// Per specification, address does not need to tell the identity of the actor,
/// Which means this is kind of ad-hoc address of continuation actor.
#[derive(Debug)]
pub enum Continuation {
    Nil,

    Reply(OneShotAny),   // type erased return
    Forward(OneShotAny), // type erased return

    #[cfg(feature = "remote")]
    BytesReply(Arc<RemotePeer>, OneShotBytes), // Serialized return
    #[cfg(feature = "remote")]
    BytesForward(Arc<RemotePeer>, OneShotBytes), // Serialized return
}

#[derive(Debug, Clone, Copy)]
pub enum Signal {
    /// Restart the actor terminating all the descendants
    Restart,
    /// Terminate the actor with all the descendants
    Terminate,
}

#[derive(Debug, Clone)]
#[cfg(feature = "remote")]
#[derive(Serialize, Deserialize)]
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
    pub fn reply(tx: OneShotAny) -> Self {
        Continuation::Reply(tx)
    }

    pub fn forward(tx: OneShotAny) -> Self {
        Continuation::Forward(tx)
    }

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
