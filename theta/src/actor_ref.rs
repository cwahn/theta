//! Actor reference types for communication with actors.
//!
//! This module provides the core communication primitives for the actor system.
//! Actor references enable type-safe, asynchronous communication between actors
//! using message passing patterns.
//!
//! # Core Types
//!
//! - [`ActorRef<A>`] - Strong reference for sending messages to actors
//! - [`WeakActorRef<A>`] - Weak reference that won't keep actors alive
//! - [`ActorHdl`] - Handle for sending control signals to actors
//! - [`MsgRequest`] - Builder for request-response messaging with timeouts
//!
//! # Communication Patterns
//!
//! ## Fire-and-forget (Tell)
//! Send a message without waiting for a response:
//! ```
//! # use theta::prelude::*;
//! # use serde::{Serialize, Deserialize};
//! # #[derive(Debug, Clone, ActorArgs)]
//! # struct MyActor;
//! # #[derive(Debug, Clone, Serialize, Deserialize)]
//! # struct MyMessage(i32);
//! # #[actor("12345678-1234-5678-9abc-123456789abc")]
//! # impl Actor for MyActor {
//! #     const _: () = {
//! #         async |MyMessage(_): MyMessage| {};
//! #     };
//! # }
//! # fn example(actor: ActorRef<MyActor>) -> Result<(), Box<dyn std::error::Error>> {
//! actor.tell(MyMessage(42))?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Request-response patterns
//! Send a message and wait for a response:
//! ```
//! # use theta::prelude::*;
//! # use serde::{Serialize, Deserialize};
//! # #[derive(Debug, Clone, ActorArgs)]
//! # struct MyActor;
//! # #[derive(Debug, Clone, Serialize, Deserialize)]
//! # struct MyMessage(i32);
//! # #[actor("12345678-1234-5678-9abc-123456789abc")]
//! # impl Actor for MyActor {
//! #     const _: () = {
//! #         async |MyMessage(_): MyMessage| -> i32 { 0 };
//! #     };
//! # }
//! # async fn example(actor: ActorRef<MyActor>) -> Result<(), Box<dyn std::error::Error>> {
//! // Basic request-response
//! let response = actor.ask(MyMessage(42)).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Forwarding pattern
//! Forward messages between actors, chaining responses back to the original sender:
//! ```
//! # use theta::prelude::*;
//! # use serde::{Serialize, Deserialize};
//! # #[derive(Debug, Clone, ActorArgs)]
//! # struct Gateway;
//! # #[derive(Debug, Clone, ActorArgs)]
//! # struct Calculator;
//! # #[derive(Debug, Clone, Serialize, Deserialize)]
//! # struct Calculate(i32, i32);
//! # #[derive(Debug, Clone, Serialize, Deserialize)]
//! # struct Return(i32);
//! # #[actor("12345678-1234-5678-9abc-123456789abc")]
//! # impl Actor for Gateway {
//! #     const _: () = {
//! #         async |msg: Calculate| -> Return {
//! #             // Gateway forwards calculation to Calculator
//! #             // Response goes directly back to original sender
//! #             Return(0) // This won't actually be used due to forwarding
//! #         };
//! #     };
//! # }
//! # #[actor("12345678-1234-5678-9abd-123456789abc")]
//! # impl Actor for Calculator {
//! #     const _: () = {
//! #         async |Calculate(a, b): Calculate| -> Return {
//! #             Return(a + b)
//! #         };
//! #         async |Return(val): Return| -> i32 {
//! #             val
//! #         };
//! #     };
//! # }
//! # fn example(gateway: ActorRef<Gateway>, calculator: ActorRef<Calculator>) -> anyhow::Result<()> {
//! // Gateway forwards Calculate message to Calculator
//! // Calculator's Return response goes back to original sender
//! gateway.forward(Calculate(5, 3), calculator)?;
//! # Ok(())
//! # }
//! ```
//!
//! # Memory Management
//!
//! - `ActorRef` keeps the actor alive (strong reference)
//! - `WeakActorRef` allows optional references without preventing cleanup
//! - Use `actor_ref.downgrade()` to convert to weak reference
//! - Use `weak_ref.upgrade()` to try converting back to strong reference

use std::{
    any::{Any, type_name},
    fmt::Debug,
    hash::Hash,
    marker::PhantomData,
    sync::Arc,
    time::Duration,
};

use futures::{
    channel::oneshot::{self, Canceled},
    future::BoxFuture,
};
use theta_flume::SendError;
use thiserror::Error;
use tokio::sync::Notify;

use crate::{
    actor::{Actor, ActorId},
    base::Ident,
    context::MonitorError,
    debug, error,
    message::{
        Continuation, Escalation, InternalSignal, Message, MsgPack, MsgTx, RawSignal, SigTx,
        WeakMsgTx, WeakSigTx,
    },
};

#[cfg(feature = "monitor")]
use crate::monitor::AnyReportTx;

#[cfg(feature = "remote")]
use {
    crate::{
        context::{LookupError, RootContext},
        remote::{
            base::{ActorTypeId, Tag},
            network::{IrohReceiver, IrohSender},
            peer::{LocalPeer, PEER, Peer},
            serde::{ForwardInfo, FromTaggedBytes, MsgPackDto},
        },
        warn,
    },
    theta_flume::unbounded_anonymous,
};

#[cfg(feature = "monitor")]
use crate::monitor::Update;

/// Trait for type-erased actor references.
///
/// This trait allows working with actor references without knowing their specific type.
/// It enables the framework to handle actors polymorphically, particularly for:
/// - Remote actor communication (serialization/deserialization)
/// - Actor monitoring and observation
/// - Dynamic actor lookup and binding
///
/// # Implementation Note
///
/// This trait is automatically implemented for all `ActorRef<A>` types.
/// Users typically don't need to interact with this trait directly.
pub trait AnyActorRef: Debug + Send + Sync + Any {
    /// Get the unique ID of the actor this reference points to.
    fn id(&self) -> ActorId;

    /// Send raw tagged bytes to the actor (used for remote communication).
    #[cfg(feature = "remote")]
    fn send_tagged_bytes(&self, tag: Tag, bytes: Vec<u8>) -> Result<(), BytesSendError>;

    /// Downcast to `&dyn Any` for type recovery.
    fn as_any(&self) -> &dyn Any;

    /// Serialize this reference for remote transmission.
    #[cfg(feature = "remote")]
    fn serialize(&self) -> Result<Vec<u8>, LookupError>;

    /// Get the task function for handling remote exports.
    #[cfg(feature = "remote")]
    fn export_task_fn(
        &self,
    ) -> fn(Peer, IrohReceiver, Arc<dyn AnyActorRef>) -> BoxFuture<'static, ()>;

    /// Set up observation of this actor via byte stream.
    #[cfg(all(feature = "remote", feature = "monitor"))]
    fn monitor_as_bytes(
        &self,
        peer: Peer,
        hdl: ActorHdl,
        bytes_tx: IrohSender,
    ) -> Result<(), MonitorError>;

    /// Get the type ID for this actor type.
    #[cfg(feature = "remote")]
    fn ty_id(&self) -> ActorTypeId;
}

/// A reference to an actor that can receive messages of type `A::Msg`.
///
/// `ActorRef` is the primary way to communicate with actors. It provides methods
/// for sending messages both fire-and-forget (`tell`) and request-response (`ask`).
///
/// # Examples
///
/// ```
/// use theta::prelude::*;
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Debug, Clone, ActorArgs)]
/// struct Counter { value: i32 }
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// struct Inc(i32);
///
/// #[actor("12345678-1234-5678-9abc-123456789abc")]
/// impl Actor for Counter {
///     const _: () = {
///         async |Inc(amount): Inc| -> i32 {
///             self.value += amount;
///             self.value
///         };
///     };
/// }
///
/// // Note: This is a compile-only example
/// fn example() -> anyhow::Result<()> {
///     // let ctx = RootContext::init_local();
///     // let actor = ctx.spawn(Counter { value: 0 });
///     // actor.tell(Inc(5))?;
///     Ok(())
/// }
/// ```
///
/// # Type Parameters
///
/// * `A` - The actor type this reference points to
#[derive(Debug)]
pub struct ActorRef<A: Actor>(pub(crate) MsgTx<A>);

/// A weak reference to an actor.
///
/// Unlike `ActorRef`, a `WeakActorRef` does not keep the actor alive.
/// This is useful for:
/// - Breaking reference cycles between parent and child actors
/// - Optional references that shouldn't prevent actor cleanup
/// - Self-references within actor contexts (`ctx.this`)
///
/// # Usage
///
/// Convert from strong to weak reference:
/// ```
/// # use theta::prelude::*;
/// # #[derive(Debug, Clone, ActorArgs)]
/// # struct MyActor;
/// # #[actor("12345678-1234-5678-9abc-123456789abc")]
/// # impl Actor for MyActor {}
/// # fn example(actor_ref: ActorRef<MyActor>) {
/// let weak_ref = actor_ref.downgrade();
/// # }
/// ```
///
/// Try to upgrade back to strong reference:
/// ```
/// # use theta::prelude::*;
/// # use serde::{Serialize, Deserialize};
/// # #[derive(Debug, Clone, ActorArgs)]
/// # struct MyActor;
/// # #[derive(Debug, Clone, Serialize, Deserialize)]
/// # struct MyMessage;
/// # #[actor("12345678-1234-5678-9abc-123456789abc")]
/// # impl Actor for MyActor {
/// #     const _: () = {
/// #         async |MyMessage: MyMessage| {};
/// #     };
/// # }
/// # fn example(weak_ref: WeakActorRef<MyActor>) -> Result<(), Box<dyn std::error::Error>> {
/// if let Some(actor_ref) = weak_ref.upgrade() {
///     // Actor is still alive, can send messages
///     actor_ref.tell(MyMessage)?;
/// }
/// # Ok(())
/// # }
/// ```
///
/// # Type Parameters
///
/// * `A` - The actor type this reference points to
#[derive(Debug)]
pub struct WeakActorRef<A: Actor>(pub(crate) WeakMsgTx<A>);

/// A handle for sending control signals to an actor.
///
/// `ActorHdl` allows sending lifecycle and supervision signals to actors
/// without being tied to a specific actor type. This enables:
/// - Parent-child supervision relationships
/// - Actor lifecycle management (start, stop, restart)
/// - Escalation handling and error propagation
///
/// # Signals
///
/// - `Signal::Terminate` - Gracefully stop the actor
/// - `Signal::Restart` - Restart the actor with fresh state
/// - Escalation signals for handling child actor failures
///
/// # Usage
///
/// Handles are typically used internally by the framework for supervision,
/// but can be accessed via actor contexts for advanced use cases.
#[derive(Debug, Clone)]
pub struct ActorHdl(pub(crate) SigTx);

/// A weak handle for actor supervision.
///
/// Similar to `WeakActorRef`, this won't keep the actor alive but allows
/// sending supervision signals.
#[derive(Debug, Clone)]
pub struct WeakActorHdl(pub(crate) WeakSigTx);

/// A builder for request-response messaging with optional timeout.
///
/// `MsgRequest` is created by calling `actor.ask(message)` and provides
/// methods for configuring request behavior before sending.
///
/// # Methods
///
/// - `await` - Send the request and wait for response (default timeout)
/// - `timeout(duration)` - Set a custom timeout before sending
///
/// # Type Parameters
///
/// * `A` - The target actor type
/// * `M` - The message type being sent
pub struct MsgRequest<'a, A, M>
where
    A: Actor,
    M: Send + Message<A> + 'static,
{
    target: &'a ActorRef<A>,
    msg: M,
}

/// A builder for sending control signals to actors.
///
/// Created internally for supervision and lifecycle management.
/// Users typically don't interact with this type directly.
pub struct SignalRequest<'a> {
    target_hdl: &'a ActorHdl,
    sig: InternalSignal,
}

/// A wrapper that adds timeout functionality to any future.
///
/// Created by calling `.timeout(duration)` on request builders.
/// Automatically cancels the operation if it takes longer than specified.
///
/// # Type Parameters
///
/// * `R` - The underlying request type
pub struct Deadline<'a, R>
where
    R: IntoFuture + Send,
{
    request: R,
    duration: Duration,
    _phantom: PhantomData<&'a ()>,
}

/// Errors that can occur when sending raw bytes to actors.
///
/// This error type is used primarily for remote actor communication
/// where messages are serialized as tagged bytes.
#[cfg(feature = "remote")]
#[derive(Debug, Error)]
pub enum BytesSendError {
    #[error(transparent)]
    DeserializeError(#[from] postcard::Error),
    #[error("send error: {0:#?}")]
    SendError((Tag, Vec<u8>)),
}

/// Errors that can occur during request-response messaging.
///
/// This encompasses all possible failure modes when using `ask()` patterns:
/// - Network/channel failures
/// - Timeouts  
/// - Actor unavailability
/// - Type mismatches
#[derive(Debug, Error)]
pub enum RequestError<T> {
    /// The receiving actor dropped the response channel
    #[error(transparent)]
    Cancelled(#[from] Canceled),
    /// Failed to send the message to the actor
    #[error(transparent)]
    SendError(#[from] SendError<T>),
    /// The actor's response channel was closed
    #[error("receiving on a closed channel")]
    RecvError,
    /// Failed to downcast the response to the expected type
    #[error("downcast failed")]
    DowncastError,
    /// Failed to deserialize the response (remote actors)
    #[cfg(feature = "remote")]
    #[error(transparent)]
    DeserializeError(#[from] postcard::Error),
    /// The request timed out waiting for a response
    #[error("timeout")]
    Timeout,
}

// Implementations

impl<A: Actor + Any> AnyActorRef for ActorRef<A> {
    fn id(&self) -> ActorId {
        self.0.id()
    }

    #[cfg(feature = "remote")]
    fn send_tagged_bytes(&self, tag: Tag, bytes: Vec<u8>) -> Result<(), BytesSendError> {
        let msg = <A::Msg as FromTaggedBytes>::from(tag, &bytes)?;

        self.send_raw(msg, Continuation::Nil)
            .map_err(|_| BytesSendError::SendError((tag, bytes)))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    #[cfg(feature = "remote")]
    fn serialize(&self) -> Result<Vec<u8>, LookupError> {
        Ok(postcard::to_stdvec(&self)?)
    }

    #[cfg(feature = "remote")]
    fn export_task_fn(
        &self,
    ) -> fn(Peer, IrohReceiver, Arc<dyn AnyActorRef>) -> BoxFuture<'static, ()> {
        |peer: Peer,
         mut in_stream: IrohReceiver,
         actor: Arc<dyn AnyActorRef>|
         -> BoxFuture<'static, ()> {
            Box::pin(PEER.scope(peer, async move {
                let Some(actor) = actor.as_any().downcast_ref::<ActorRef<A>>() else {
                    return crate::error!(
                        "Failed to downcast any actor reference to {}",
                        type_name::<A>()
                    );
                };

                loop {
                    let bytes = match in_stream.recv_frame().await {
                        Ok(bytes) => bytes,
                        Err(e) => break crate::error!("Failed to receive frame from stream: {e}"),
                    };

                    let (msg, k_dto) = match postcard::from_bytes::<MsgPackDto<A>>(&bytes) {
                        Ok(msg_k_dto) => msg_k_dto,
                        Err(e) => {
                            warn!("Failed to deserialize msg pack dto: {e}");
                            continue;
                        }
                    };

                    let (msg, k): MsgPack<A> = (msg, k_dto.into());

                    if let Err(e) = actor.send_raw(msg, k) {
                        break crate::error!("Failed to send message to actor: {e}");
                    }
                }
            }))
        }
    }

    #[cfg(all(feature = "remote", feature = "monitor"))]
    fn monitor_as_bytes(
        &self,
        peer: Peer,
        hdl: ActorHdl,
        mut bytes_tx: IrohSender,
    ) -> Result<(), MonitorError> {
        let (tx, rx) = unbounded_anonymous::<Update<A>>();

        tokio::spawn(PEER.scope(peer, async move {
            loop {
                let Some(report) = rx.recv().await else {
                    return warn!("Report channel is closed");
                };

                let bytes = match postcard::to_stdvec(&report) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        warn!("Failed to serialize report: {e}");
                        continue;
                    }
                };

                if let Err(e) = bytes_tx.send_frame(bytes).await {
                    break warn!("Failed to send report frame: {e}");
                }
            }
        }));

        hdl.monitor(Box::new(tx))
            .map_err(|_| MonitorError::SigSendError)?;

        Ok(())
    }

    #[cfg(feature = "remote")]
    fn ty_id(&self) -> ActorTypeId {
        A::IMPL_ID
    }
}

// impl<A> AnyActorRef for WeakActorRef<A> where A: Actor + Any {}

impl<A> ActorRef<A>
where
    A: Actor,
{
    /// Get the unique identifier of the actor this reference points to.
    ///
    /// Get the unique ID of this actor.
    ///
    /// # Returns
    ///
    /// An `ActorId` that uniquely identifies this actor instance within the system.
    pub fn id(&self) -> ActorId {
        self.0.id()
    }

    /// Get the actor's identifier as a byte slice for binding and lookup.
    ///
    /// # Returns
    ///
    /// An `Ident` containing the actor ID in bytes format for registry operations.
    pub fn ident(&self) -> Ident {
        self.0.id().to_bytes_le().to_vec().into()
    }

    /// Check if this reference points to a nil/null actor.
    ///
    /// # Returns
    ///
    /// `true` if this reference points to a nil actor (invalid or uninitialized reference),
    /// `false` if it points to a valid actor instance.
    ///
    /// # Usage
    /// Check if this is a nil (invalid) actor reference.
    ///
    /// # Returns
    ///
    /// `true` if this reference is nil, `false` otherwise.
    pub fn is_nil(&self) -> bool {
        self.0.id().is_nil()
    }

    /// Send a fire-and-forget message to the actor.
    ///
    /// # Arguments
    ///
    /// * `msg` - The message to send to the actor
    ///
    /// # Returns
    ///
    /// `Result<(), SendError>` - Success or error if actor's mailbox is closed
    ///
    /// # Errors
    ///
    /// Returns `SendError` if the actor's mailbox is closed or full.
    pub fn tell<M>(&self, msg: M) -> Result<(), SendError<(A::Msg, Continuation)>>
    where
        M: Message<A>,
    {
        self.send_raw(msg.into(), Continuation::Nil)
    }

    /// Send a request-response message to the actor.
    ///
    /// # Arguments
    ///
    /// * `msg` - The message to send that expects a response
    ///
    /// # Returns
    ///
    /// `MsgRequest` builder for configuring timeout before awaiting the response.
    ///
    /// # Note
    ///
    /// Always use timeouts to prevent hanging: `actor.ask(msg).timeout(duration).await`
    pub fn ask<M>(&self, msg: M) -> MsgRequest<'_, A, M>
    where
        M: Message<A>,
    {
        MsgRequest { target: self, msg }
    }

    /// Forward a message to another actor, chaining the response back to original sender.
    ///
    /// # Arguments
    ///
    /// * `msg` - The message to forward
    /// * `target` - The actor to forward the message to
    ///
    /// # Returns
    ///
    /// `Result<(), SendError>` - Success or error if actor's mailbox is closed
    ///
    /// # Errors
    ///
    /// Returns `SendError` if this actor's mailbox is closed or full.
    pub fn forward<M, B>(
        &self,
        msg: M,
        target: ActorRef<B>,
    ) -> Result<(), SendError<(A::Msg, Continuation)>>
    where
        M: Message<A>,
        B: Actor,
        <M as Message<A>>::Return: Message<B>,
    {
        let (tx, rx) = oneshot::channel::<Box<dyn Any + Send>>();

        tokio::spawn(async move {
            let Ok(ret) = rx.await else {
                return; // Cancelled
            };

            let b_msg = match ret.downcast::<<M as Message<A>>::Return>() {
                Ok(b_msg) => b_msg,
                Err(ret) => {
                    #[cfg(not(feature = "remote"))]
                    {
                        return crate::error!(
                            "Failed to downcast response from actor {}: expected {}",
                            target.id(),
                            type_name::<<M as Message<A>>::Return>()
                        );
                    }

                    #[cfg(feature = "remote")]
                    {
                        let Ok(tx) = ret.downcast::<oneshot::Sender<ForwardInfo>>() else {
                            return crate::error!(
                                "Failed to downcast initial response from actor {}: expected {} or oneshot::Sender<ForwardInfo>",
                                target.id(),
                                type_name::<<M as Message<A>>::Return>()
                            );
                        };

                        let forward_info = match LocalPeer::inst().get_import::<B>(target.id()) {
                            None => {
                                if !RootContext::is_bound_impl::<B>(&target.ident()) {
                                    // ! Once exported, never get freed and dropped.
                                    RootContext::bind_impl(target.ident().clone(), target.clone());
                                }

                                // Local is always second_party remote with respect to the recipient
                                ForwardInfo::Remote {
                                    public_key: None,
                                    ident: target.ident(),
                                    tag: <<M as Message<A>>::Return as Message<B>>::TAG,
                                }
                            }
                            Some(import) => ForwardInfo::Remote {
                                public_key: Some(import.peer.public_key()),
                                ident: target.ident(),
                                tag: <<M as Message<A>>::Return as Message<B>>::TAG,
                            },
                        };

                        if tx.send(forward_info).is_err() {
                            error!("Failed to send forward info");
                        }

                        return debug!("Deligate forwarding task to actor {}", target.id());
                    }
                }
            };

            let _ = target.send_raw((*b_msg).into(), Continuation::Nil);
        });

        let continuation = Continuation::forward(tx);

        self.send_raw(msg.into(), continuation)
    }

    /// Convert this strong reference to a weak reference.
    ///
    /// # Returns
    ///
    /// `WeakActorRef<A>` that doesn't keep the actor alive.
    pub fn downgrade(&self) -> WeakActorRef<A> {
        WeakActorRef(self.0.downgrade())
    }

    /// Check if the actor's message channel is closed (actor has terminated).
    ///
    /// # Returns
    ///
    /// `true` if the actor has terminated, `false` if still running.
    pub fn is_closed(&self) -> bool {
        self.0.is_closed()
    }

    pub(crate) fn send_raw(
        &self,
        msg: A::Msg,
        k: Continuation,
    ) -> Result<(), SendError<(A::Msg, Continuation)>> {
        self.0.send((msg, k))
    }
}

impl<A> Clone for ActorRef<A>
where
    A: Actor,
{
    fn clone(&self) -> Self {
        ActorRef(self.0.clone())
    }
}

impl<A> PartialEq for ActorRef<A>
where
    A: Actor,
{
    fn eq(&self, other: &Self) -> bool {
        self.0.id() == other.0.id()
    }
}

impl<A> Eq for ActorRef<A> where A: Actor {}

impl<A> Hash for ActorRef<A>
where
    A: Actor,
{
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.id().hash(state);
    }
}

impl<A> PartialOrd for ActorRef<A>
where
    A: Actor,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.0.id().cmp(&other.0.id()))
    }
}

impl<A> WeakActorRef<A>
where
    A: Actor,
{
    /// Attempt to upgrade this weak reference to a strong reference.
    ///
    /// # Returns
    ///
    /// `Some(ActorRef<A>)` if the actor is still alive, `None` if terminated.
    pub fn upgrade(&self) -> Option<ActorRef<A>> {
        self.0.upgrade().map(|tx| ActorRef(tx))
    }
}

impl<A> Clone for WeakActorRef<A>
where
    A: Actor,
{
    fn clone(&self) -> Self {
        WeakActorRef(self.0.clone())
    }
}

impl ActorHdl {
    #[allow(dead_code)]
    pub(crate) fn id(&self) -> ActorId {
        self.0.id()
    }

    pub(crate) fn signal(&self, sig: InternalSignal) -> SignalRequest<'_> {
        SignalRequest {
            target_hdl: self,
            sig,
        }
    }

    pub(crate) fn downgrade(&self) -> WeakActorHdl {
        WeakActorHdl(self.0.downgrade())
    }

    pub(crate) fn escalate(
        &self,
        this_hdl: ActorHdl,
        escalation: Escalation,
    ) -> Result<(), SendError<RawSignal>> {
        self.raw_send(RawSignal::Escalation(this_hdl, escalation))
    }

    #[cfg(feature = "monitor")]
    pub(crate) fn monitor(&self, tx: AnyReportTx) -> Result<(), SendError<RawSignal>> {
        self.raw_send(RawSignal::Monitor(tx))
    }

    pub(crate) fn raw_send(&self, raw_sig: RawSignal) -> Result<(), SendError<RawSignal>> {
        self.0.send(raw_sig)
    }
}

impl PartialEq for ActorHdl {
    fn eq(&self, other: &Self) -> bool {
        self.0.same_channel(&other.0)
    }
}

impl WeakActorHdl {
    pub(crate) fn upgrade(&self) -> Option<ActorHdl> {
        Some(ActorHdl(self.0.upgrade()?))
    }
}

impl<'a, A, M> MsgRequest<'a, A, M>
where
    A: Actor,
    M: Message<A>,
{
    /// Set a timeout for this message request.
    ///
    /// # Arguments
    ///
    /// * `duration` - Maximum time to wait for a response
    ///
    /// # Returns
    ///
    /// `Deadline` that will timeout if the actor doesn't respond within the duration.
    pub fn timeout(self, duration: Duration) -> Deadline<'a, Self> {
        Deadline {
            request: self,
            duration,
            _phantom: PhantomData,
        }
    }
}

impl<'a, A, M> IntoFuture for MsgRequest<'a, A, M>
where
    A: Actor,
    M: Message<A>,
{
    type Output = Result<<M as Message<A>>::Return, RequestError<MsgPack<A>>>;
    type IntoFuture = BoxFuture<'a, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();

            self.target
                .0
                .send((self.msg.into(), Continuation::reply(tx)))?;

            let ret = rx.await?;

            match ret.downcast::<M::Return>() {
                Ok(res) => Ok(*res), // Local reply
                Err(ret) => {
                    #[cfg(not(feature = "remote"))]
                    {
                        return Err(RequestError::DowncastError);
                    }
                    #[cfg(feature = "remote")]
                    {
                        let Ok(remote_reply_rx) =
                            ret.downcast::<oneshot::Receiver<(Peer, Vec<u8>)>>()
                        else {
                            return Err(RequestError::DowncastError);
                        };

                        let (peer, bytes) = remote_reply_rx.await?;
                        let res =
                            PEER.sync_scope(peer, || postcard::from_bytes::<M::Return>(&bytes))?;

                        Ok(res)
                    }
                }
            }
        })
    }
}

impl<'a> SignalRequest<'a> {
    /// Set a timeout for this signal request.
    ///
    /// # Arguments
    ///
    /// * `duration` - Maximum time to wait for signal acknowledgment
    ///
    /// # Returns
    ///
    /// `Deadline` that will timeout if the signal isn't acknowledged within the duration.
    pub fn timeout(self, duration: Duration) -> Deadline<'a, Self> {
        Deadline {
            request: self,
            duration,
            _phantom: PhantomData,
        }
    }
}

impl<'a> IntoFuture for SignalRequest<'a> {
    type Output = Result<(), RequestError<RawSignal>>;
    type IntoFuture = BoxFuture<'a, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let k = Arc::new(Notify::new());

            let raw_sig = match self.sig {
                InternalSignal::Pause => RawSignal::Pause(Some(k.clone())),
                InternalSignal::Resume => RawSignal::Resume(Some(k.clone())),
                InternalSignal::Restart => RawSignal::Restart(Some(k.clone())),
                InternalSignal::Terminate => RawSignal::Terminate(Some(k.clone())),
            };

            self.target_hdl.raw_send(raw_sig)?;

            k.notified().await;

            Ok(())
        })
    }
}

impl<'a, R, M> IntoFuture for Deadline<'a, R>
where
    R: 'a + IntoFuture<Output = Result<M, RequestError<M>>> + Send,
    R::IntoFuture: Send, // Add this bound
{
    type Output = R::Output;
    type IntoFuture = BoxFuture<'a, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let res = tokio::time::timeout(self.duration, self.request).await;
            match res {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(RequestError::Timeout),
            }
        })
    }
}
