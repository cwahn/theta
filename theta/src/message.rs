//! Message types and traits for actor communication.
//!
//! This module defines the core message system used by actors to communicate.
//! The main trait is [`Message`] which defines how messages are processed.

use std::{any::Any, fmt::Debug, future::Future, sync::Arc};

#[cfg(not(feature = "remote"))]
use std::panic::UnwindSafe;

use futures::channel::oneshot;
use theta_flume::{Receiver, Sender, WeakSender};
use tokio::sync::Notify;

#[cfg(feature = "remote")]
use crate::remote::base::Key;
use crate::{actor::Actor, actor_ref::ActorHdl, context::Context};

#[cfg(feature = "monitor")]
use crate::monitor::AnyUpdateTx;

#[cfg(feature = "remote")]
use {
    crate::remote::{
        base::Tag,
        peer::{PEER, Peer},
    },
    serde::{Deserialize, Serialize},
};

/// A message pack containing a message and its continuation.
pub type MsgPack<A> = (<A as Actor>::Msg, Continuation);

/// One-shot sender for type-erased values in actor responses.
pub type OneShotAny = oneshot::Sender<Box<dyn Any + Send>>;

/// One-shot sender for serialized bytes in remote responses.
pub type OneShotBytes = oneshot::Sender<Vec<u8>>;

/// Channel for sending messages to actors.
pub type MsgTx<A> = Sender<MsgPack<A>>;

/// Weak reference to message sender that won't prevent actor shutdown.
pub type WeakMsgTx<A> = WeakSender<MsgPack<A>>;

/// Channel for receiving messages in actor mailboxes.
pub type MsgRx<A> = Receiver<MsgPack<A>>;

/// Channel for sending shutdown signals to actors.
pub type SigTx = Sender<RawSignal>;

/// Weak reference to signal sender that won't prevent actor shutdown.
pub type WeakSigTx = WeakSender<RawSignal>;

/// Channel for receiving shutdown signals in actors.
pub type SigRx = Receiver<RawSignal>;

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

    /// Returned buffer will be reused for next serialization
    #[cfg(feature = "remote")]
    fn process_to_bytes(
        state: &mut A,
        ctx: Context<A>,
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

    // ? Will it not cause any problem between Thetas with different features?
    #[cfg(feature = "remote")]
    // BytesReply(Peer, OneShotBytes), // Serialized return
    BinReply {
        peer: Peer,
        key: Key,
    },
    // #[cfg(feature = "remote")]
    // BytesForward(Peer, OneShotBytes), // Serialized return
    // BytesForward(Peer, Ident, Tag),
    // ? Two kinds of byte forward?
    #[cfg(feature = "remote")]
    LocalBinForward {
        peer: Peer,
        tx: OneShotBytes,
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

    Pause(Option<Arc<Notify>>),
    Resume(Option<Arc<Notify>>),
    Restart(Option<Arc<Notify>>),
    Terminate(Option<Arc<Notify>>),
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
    /// # Returns
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
    /// # Returns
    ///
    /// `Continuation::Forward` variant for message forwarding.
    pub fn forward(tx: OneShotAny) -> Self {
        Continuation::Forward(tx)
    }

    /// Check if this continuation is nil (no response expected).
    ///
    /// # Returns
    ///
    /// `true` if this is a nil continuation, `false` otherwise.
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
