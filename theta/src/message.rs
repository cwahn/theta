use core::any::Any;

#[cfg(feature = "std")]
use std::{boxed::Box, fmt::Debug, sync::Arc};

#[cfg(feature = "alloc")]
use alloc::{boxed::Box, sync::Arc};

use futures::channel::oneshot;
use tokio::sync::{
    // todo Remove dependency of Notify
    Notify,
    mpsc::{UnboundedReceiver, UnboundedSender, WeakUnboundedSender},
};

use crate::{actor::Actor, monitor::AnyReportTx};

// pub type DynMessage<A> = Box<dyn Message<A>>;
pub type MsgPack<A: Actor> = (A::Msg, Continuation);

// todo Remove oneshot dependency
pub type OneShot = oneshot::Sender<Box<dyn Any + Send>>;

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

    pub(crate) fn send_and_forget(self, res: Box<dyn Any + Send>) {
        if let Err(_) = self.send(res) {
            #[cfg(feature = "tracing")]
            tracing::error!("Failed to send response");
        }
    }
}

// impl From<Signal> for InternalSignal {
//     fn from(signal: Signal) -> Self {
//         match signal {
//             Signal::Terminate => InternalSignal::Terminate,
//         }
//     }
// }

pub struct PoisonPill {}

// #[derive(Debug, Clone, Copy)]
// pub enum Signal {
//     // Restart,
//     Terminate,
// }

// #[derive(Debug, Clone)]
// pub enum Escalation {
//     Initialize(String),
//     ProcessMsg(String),
//     Supervise(String),
// }

#[derive(Debug)]
pub enum RawSignal {
    Observe(AnyReportTx),

    // Escalation(ActorHdl, Escalation),
    // ChildDropped,
    // Pause(Option<Arc<Notify>>),
    // Resume(Option<Arc<Notify>>),
    // Restart(Option<Arc<Notify>>),
    Terminate(Option<Arc<Notify>>),
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum InternalSignal {
    // Pause,
    // Resume,
    // Restart,
    Terminate,
}
