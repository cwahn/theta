//! Message types and traits for actor communication.
//!
//! This module defines the core message system used by actors to communicate.
//! The main trait is [`Message`] which defines how messages are processed.

use std::{any::Any, fmt::Debug, future::Future};

#[cfg(not(feature = "remote"))]
use std::panic::UnwindSafe;

use futures::channel::oneshot;
use theta_flume::{Receiver, Sender, WeakSender};

use crate::{actor::Actor, actor_ref::ActorHdl, context::Context};

#[cfg(feature = "monitor")]
use crate::monitor::AnyUpdateTx;

#[cfg(feature = "remote")]
use {
    crate::remote::{
        base::Tag,
        peer::{PEER, Peer, RemoteSender},
    },
    serde::{Deserialize, Serialize},
    std::sync::Arc,
};

/// One-shot acknowledgment sender for signal completion.
pub(crate) type SigAck = oneshot::Sender<()>;

/// A message pack containing a message and its continuation.
pub type MsgPack<A> = (<A as Actor>::Msg, Continuation);

/// One-shot sender for type-erased values in actor responses.
pub type OneShotAny = oneshot::Sender<Box<dyn Any + Send>>;

/// One-shot sender for serialized bytes in remote responses.
pub type OneShotBytes = oneshot::Sender<Vec<u8>>;

/// Channel for sending messages to actors.
pub type MsgTx<A> = MsgSender<A>;

/// Weak reference to message sender that won't prevent actor shutdown.
pub type WeakMsgTx<A> = WeakMsgSender<A>;

/// Channel for receiving messages in actor mailboxes.
pub type MsgRx<A> = Receiver<MsgPack<A>>;

/// Channel for sending shutdown signals to actors.
pub type SigTx = Sender<RawSignal>;

/// Weak reference to signal sender that won't prevent actor shutdown.
pub type WeakSigTx = WeakSender<RawSignal>;

/// Channel for receiving shutdown signals in actors.
pub type SigRx = Receiver<RawSignal>;

// === MsgSender: unified sender for local and remote actors ===

/// A message sender that works for both local and remote actors.
/// Local: wraps theta_flume::Sender directly.
/// Remote: eagerly serializes and buffers for flush pool workers.
pub enum MsgSender<A: Actor> {
    Local(Sender<MsgPack<A>>),
    #[cfg(feature = "remote")]
    Remote(RemoteSender<A>),
}

impl<A: Actor> MsgSender<A> {
    pub fn send(&self, msg: MsgPack<A>) -> Result<(), theta_flume::SendError<MsgPack<A>>> {
        match self {
            Self::Local(tx) => tx.send(msg),
            #[cfg(feature = "remote")]
            Self::Remote(r) => r.send(msg),
        }
    }

    pub fn is_closed(&self) -> bool {
        match self {
            Self::Local(tx) => tx.is_closed(),
            #[cfg(feature = "remote")]
            Self::Remote(r) => r.mailbox.closed.load(std::sync::atomic::Ordering::Acquire),
        }
    }

    pub fn id(&self) -> uuid::Uuid {
        match self {
            Self::Local(tx) => tx.id(),
            #[cfg(feature = "remote")]
            Self::Remote(r) => r.mailbox.uuid,
        }
    }

    pub fn ptr_id(&self) -> usize {
        match self {
            Self::Local(tx) => tx.ptr_id(),
            #[cfg(feature = "remote")]
            Self::Remote(r) => Arc::as_ptr(&r.mailbox) as usize,
        }
    }

    pub fn downgrade(&self) -> WeakMsgSender<A> {
        match self {
            Self::Local(tx) => WeakMsgSender::Local(tx.downgrade()),
            #[cfg(feature = "remote")]
            Self::Remote(r) => WeakMsgSender::Remote(Arc::downgrade(&r.mailbox)),
        }
    }

    pub fn sender_count(&self) -> usize {
        match self {
            Self::Local(tx) => tx.sender_count(),
            #[cfg(feature = "remote")]
            Self::Remote(r) => r.mailbox.sender_count.load(std::sync::atomic::Ordering::Relaxed),
        }
    }
}

impl<A: Actor> Clone for MsgSender<A> {
    fn clone(&self) -> Self {
        match self {
            Self::Local(tx) => Self::Local(tx.clone()),
            #[cfg(feature = "remote")]
            Self::Remote(r) => Self::Remote(r.clone()),
        }
    }
}

impl<A: Actor> Debug for MsgSender<A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local(tx) => write!(f, "MsgSender::Local({:?})", tx.id()),
            #[cfg(feature = "remote")]
            Self::Remote(r) => write!(f, "MsgSender::Remote({:?})", r),
        }
    }
}

// === WeakMsgSender ===

pub enum WeakMsgSender<A: Actor> {
    Local(WeakSender<MsgPack<A>>),
    #[cfg(feature = "remote")]
    Remote(std::sync::Weak<crate::remote::peer::ImportMailbox>),
}

impl<A: Actor> WeakMsgSender<A> {
    pub fn ptr_id(&self) -> usize {
        match self {
            Self::Local(w) => w.ptr_id(),
            #[cfg(feature = "remote")]
            Self::Remote(w) => w.as_ptr() as usize,
        }
    }

    pub fn upgrade(&self) -> Option<MsgSender<A>> {
        match self {
            Self::Local(w) => w.upgrade().map(MsgSender::Local),
            #[cfg(feature = "remote")]
            Self::Remote(w) => {
                use std::marker::PhantomData;
                use std::sync::atomic::Ordering;
                w.upgrade().map(|arc| {
                    arc.sender_count.fetch_add(1, Ordering::Relaxed);
                    MsgSender::Remote(RemoteSender {
                        mailbox: arc,
                        _phantom: PhantomData,
                    })
                })
            }
        }
    }
}

impl<A: Actor> Clone for WeakMsgSender<A> {
    fn clone(&self) -> Self {
        match self {
            Self::Local(w) => Self::Local(w.clone()),
            #[cfg(feature = "remote")]
            Self::Remote(w) => Self::Remote(w.clone()),
        }
    }
}

impl<A: Actor> Debug for WeakMsgSender<A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local(w) => write!(f, "WeakMsgSender::Local({:?})", w.ptr_id()),
            #[cfg(feature = "remote")]
            Self::Remote(w) => write!(f, "WeakMsgSender::Remote({:?})", w.as_ptr()),
        }
    }
}

/// Trait for messages that can be sent to actors.
///
/// This trait is typically implemented automatically by the `#[actor]` macro
/// for each message type defined in the actor's behavior block.
///
/// # Type Parameters
///
/// * `A` - The actor type that can handle this message
pub trait Message<A: Actor>: Debug + Send + Into<A::Msg> + 'static {
    /// The return type when processing this message.
    ///
    /// For local-only actors, this can be any `Send + UnwindSafe` type.
    /// For remote-capable actors, it must also be serializable.
    #[cfg(not(feature = "remote"))]
    type Return: Send + UnwindSafe + 'static;

    #[cfg(feature = "remote")]
    type Return: Debug + Send + Sync + Serialize + for<'de> Deserialize<'de> + 'static;

    /// Tag used for identifying this message type in remote communication.
    #[cfg(feature = "remote")]
    const TAG: Tag;

    /// Process this message with the given actor state.
    ///
    /// This method is called when the message is delivered to an actor.
    /// It should not be called directly by user code.
    fn process(
        state: &mut A,
        ctx: &Context<A>,
        msg: Self,
    ) -> impl Future<Output = Self::Return> + Send;

    fn process_to_any(
        state: &mut A,
        ctx: &Context<A>,
        msg: Self,
    ) -> impl Future<Output = Box<dyn Any + Send>> + Send {
        async move { Box::new(Self::process(state, ctx, msg).await) as Box<dyn Any + Send> }
    }

    /// Returned buffer will be reused for next serialization
    #[cfg(feature = "remote")]
    fn process_to_bytes(
        state: &mut A,
        ctx: &Context<A>,
        peer: Peer,
        msg: Self,
    ) -> impl Future<Output = Result<Vec<u8>, postcard::Error>> + Send {
        async move {
            let ret = Self::process(state, ctx, msg).await;

            PEER.sync_scope(peer, || postcard::to_stdvec(&ret))
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
    BinReply {
        peer: Peer,
        reply_tx: oneshot::Sender<Vec<u8>>,
    },
    #[cfg(feature = "remote")]
    LocalBinForward {
        peer: Peer,
        tx: OneShotBytes, // Relatively rare, take channel cost instead of larger continuation size
    },
    #[cfg(feature = "remote")]
    RemoteBinForward {
        peer: Peer,
        tx: OneShotBytes, // Relatively rare, take channel cost instead of larger continuation size
    },
}

/// Actor lifecycle control signals.
#[derive(Debug, Clone, Copy)]
pub enum Signal {
    /// Restart the actor terminating all the descendants
    Restart,
    /// Terminate the actor with all the descendants
    Terminate,
}

/// Error information for actor supervision and error handling.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "remote", derive(Serialize, Deserialize))]
pub enum Escalation {
    Initialize(String),
    ProcessMsg(String),
    Supervise(String),
}

/// Internal supervision and control signals for actor management.
#[derive(Debug)]
pub enum RawSignal {
    #[cfg(feature = "monitor")]
    Monitor(AnyUpdateTx),

    Escalation(ActorHdl, Escalation),
    ChildDropped,

    Pause(Option<SigAck>),
    Resume(Option<SigAck>),
    Restart(Option<SigAck>),
    Terminate(Option<SigAck>),
}

/// Internal signal variants without notification channels.
#[derive(Debug, Clone, Copy)]
pub(crate) enum InternalSignal {
    Pause,
    Resume,
    Restart,
    Terminate,
}

// Implementations

impl Continuation {
    /// Create a reply continuation with the given response channel.
    ///
    /// # Arguments
    ///
    /// * `tx` - Channel for sending the reply
    ///
    /// # Return
    ///
    /// `Continuation::Reply` variant for direct responses.
    pub fn reply(tx: OneShotAny) -> Self {
        Continuation::Reply(tx)
    }

    /// Create a forward continuation with the given response channel.
    ///
    /// # Arguments
    ///
    /// * `tx` - Channel for forwarding the response
    ///
    /// # Return
    ///
    /// `Continuation::Forward` variant for message forwarding.
    pub fn forward(tx: OneShotAny) -> Self {
        Continuation::Forward(tx)
    }

    /// Check if this continuation is nil (no response expected).
    ///
    /// # Return
    ///
    /// `true` if this is a nil continuation, `false` otherwise.
    pub fn is_nil(&self) -> bool {
        matches!(self, Continuation::Nil)
    }
}

impl InternalSignal {
    pub(crate) fn into_raw(self, k: Option<SigAck>) -> RawSignal {
        match self {
            InternalSignal::Pause => RawSignal::Pause(k),
            InternalSignal::Resume => RawSignal::Resume(k),
            InternalSignal::Restart => RawSignal::Restart(k),
            InternalSignal::Terminate => RawSignal::Terminate(k),
        }
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
