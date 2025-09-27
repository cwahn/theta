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
//! - Use `actor.downgrade()` to convert to weak reference
//! - Use `weak_actor.upgrade()` to try converting back to strong reference
use std::{
    any::{Any, type_name},
    fmt::{Debug, Display},
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
use tracing::error;

use crate::{
    actor::{Actor, ActorId},
    base::{BindingError, Hex, Ident, parse_ident},
    context::BINDINGS,
    message::{
        Continuation, Escalation, InternalSignal, Message, MsgPack, MsgTx, RawSignal, SigTx,
        WeakMsgTx, WeakSigTx,
    },
};

#[cfg(feature = "monitor")]
use {
    crate::base::MonitorError,
    crate::monitor::{AnyUpdateTx, UpdateTx},
};

#[cfg(feature = "remote")]
use {
    crate::{
        prelude::RemoteError,
        remote::{
            base::{ActorTypeId, Tag, parse_url},
            network::RxStream,
            peer::{LocalPeer, PEER, Peer},
            serde::{ForwardInfo, FromTaggedBytes, MsgPackDto},
        },
    },
    iroh::PublicKey,
    tracing::{debug, trace, warn},
    url::Url,
};

#[cfg(all(feature = "remote", feature = "monitor"))]
use {
    crate::{monitor::Update, remote::network::TxStream},
    theta_flume::unbounded_anonymous,
    tracing::level_filters::STATIC_MAX_LEVEL,
};

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
    ///
    /// # Return
    ///
    /// An `Option<ActorId>` which is None if the actor is dropped.
    fn id(&self) -> Option<ActorId>;

    /// Send raw tagged bytes to the actor (used for remote communication).
    #[cfg(feature = "remote")]
    fn send_tagged_bytes(&self, tag: Tag, bytes: Vec<u8>) -> Result<(), BytesSendError>;

    /// Downcast to `&dyn Any` for type recovery.
    ///
    /// # Return
    ///
    /// An `Option<&dyn Any>` which is None if the actor is dropped.
    fn as_any(&self) -> Option<Box<dyn Any>>;

    /// Serialize this reference for remote transmission.
    #[cfg(feature = "remote")]
    fn serialize(&self) -> Result<Vec<u8>, BindingError>;

    #[cfg(feature = "remote")]
    fn spawn_export_task(&self, peer: Peer, in_stream: RxStream) -> Result<(), BindingError>;

    /// Set up observation of this actor via byte stream.
    #[cfg(all(feature = "remote", feature = "monitor"))]
    fn monitor_as_bytes(
        &self,
        peer: Peer,
        hdl: ActorHdl,
        tx_stream: TxStream,
    ) -> Result<(), MonitorError>;

    /// Get the type ID for this actor type.
    #[cfg(feature = "remote")]
    fn ty_id(&self) -> ActorTypeId;

    #[cfg(feature = "remote")]
    fn sender_count(&self) -> usize;
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
/// # fn example(actor: ActorRef<MyActor>) {
/// let weak_actor = actor.downgrade();
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
/// # fn example(weak_actor: WeakActorRef<MyActor>) -> Result<(), Box<dyn std::error::Error>> {
/// if let Some(actor) = weak_actor.upgrade() {
///     // Actor is still alive, can send messages
///     actor.tell(MyMessage)?;
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
    #[error(transparent)]
    SendError(#[from] SendError<(Tag, Vec<u8>)>),
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

// #[cfg(feature = "remote")]
// #[derive(Debug, Clone)]
// pub(crate) struct ExportedActorRef<A: Actor> {
//     inner: Arc<ExportedActorRefInner<A>>,
// }

// #[cfg(feature = "remote")]
// #[derive(Debug, Error)]
// struct ExportedActorRefInner<A: Actor> {
//     actor: WeakActorRef<A>,
//     actor_id: ActorId,
// }

// Implementations

impl<A> ActorRef<A>
where
    A: Actor,
{
    /// Get the unique identifier of the actor this reference points to.
    ///
    /// Get the unique ID of this actor.
    ///
    /// # Return
    ///
    /// An `ActorId` that uniquely identifies this actor instance within the system.
    pub fn id(&self) -> ActorId {
        self.0.id()
    }

    /// Get the unique identifier for the actor reference instance.
    ///
    /// # Return
    ///
    /// A `u64` hash representing this specific `ActorRef` instance.
    pub fn ref_id(&self) -> usize {
        self.0.ptr_id()
    }

    /// Get the actor's identifier as a byte slice for binding and lookup.
    ///
    /// # Return
    ///
    /// An `Ident` containing the actor ID in bytes format for registry operations.
    pub fn ident(&self) -> Ident {
        *self.0.id().as_bytes()
    }

    /// Check if this reference points to a nil/null actor.
    ///
    /// # Return
    ///
    /// `true` if this reference points to a nil actor (invalid or uninitialized reference),
    /// `false` if it points to a valid actor instance.
    ///
    /// # Usage
    /// Check if this is a nil (invalid) actor reference.
    ///
    /// # Return
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
    /// # Return
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
        self.send(msg.into(), Continuation::Nil)
    }

    /// Send a request-response message to the actor.
    ///
    /// # Arguments
    ///
    /// * `msg` - The message to send that expects a response
    ///
    /// # Return
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
    /// # Return
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
                Err(_ret) => {
                    #[cfg(not(feature = "remote"))]
                    {
                        return error!(
                            %target,
                            expected = type_name::<<M as Message<A>>::Return>(),
                            "failed to downcast forwarded msg"
                        );
                    }

                    #[cfg(feature = "remote")]
                    {
                        let Ok(tx) = _ret.downcast::<oneshot::Sender<ForwardInfo>>() else {
                            return error!(
                                %target,
                                expected = format!(
                                    "{} or oneshot::Sender<ForwardInfo>",
                                    type_name::<<M as Message<A>>::Return>()
                                ),
                                "failed to downcast forwarded msg"
                            );
                        };

                        let forward_info = ForwardInfo {
                            actor_id: target.id(),
                            tag: <<M as Message<A>>::Return as Message<B>>::TAG,
                        };

                        if tx.send(forward_info).is_err() {
                            return error!(
                                %target,
                                "failed to send forward info"
                            );
                        }

                        return debug!(
                            %target,
                            "delegated forwarding",
                        );
                    }
                }
                Ok(b_msg) => b_msg,
            };

            let _ = target.send((*b_msg).into(), Continuation::Nil);
        });

        let continuation = Continuation::forward(tx);

        self.send(msg.into(), continuation)
    }

    /// Send a message and a continuation to the actor.
    ///
    /// # Arguments
    ///
    /// * `msg` - The message to send.
    /// * `k` - The continuation to execute after sending the message.
    ///
    /// # Return
    ///
    /// `Result<(), SendError<(A::Msg, Continuation)>>` indicating success or failure.
    ///
    /// # Note
    ///
    /// This is the underlying raw operation of `tell`, `ask`, and `forward`
    pub fn send(
        &self,
        msg: A::Msg,
        k: Continuation,
    ) -> Result<(), SendError<(A::Msg, Continuation)>> {
        self.0.send((msg, k))
    }

    /// Convert this strong reference to a weak reference.
    ///
    /// # Return
    ///
    /// `WeakActorRef<A>` that doesn't keep the actor alive.
    pub fn downgrade(&self) -> WeakActorRef<A> {
        WeakActorRef(self.0.downgrade())
    }

    /// Check if the actor's message channel is closed (actor has terminated).
    ///
    /// # Return
    ///
    /// `true` if the actor has terminated, `false` if still running.
    pub fn is_closed(&self) -> bool {
        self.0.is_closed()
    }

    /// Monitor this actor for state and status updates.
    ///
    /// Establishes monitoring for this actor instance, automatically determining
    /// whether the actor is local or remote and routing the monitoring request
    /// appropriately through the actor's unique ID.
    ///
    /// # Arguments
    ///
    /// * `tx` - Channel to send updates to
    ///
    /// # Return
    ///
    /// `Result<(), RemoteError>` - Success or error during observation setup
    ///
    /// # Errors
    ///
    /// Returns `RemoteError` if:
    /// - Actor is not found locally or remotely
    /// - Network connection to remote peer fails (for remote actors)
    /// - Failed to send observation signal to actor
    #[cfg(all(feature = "monitor", feature = "remote"))]
    pub async fn monitor(&self, tx: UpdateTx<A>) -> Result<(), RemoteError> {
        match LocalPeer::inst().get_import_public_key(&self.id()) {
            None => self.monitor_local(tx).map_err(RemoteError::MonitorError),
            Some(public_key) => self.monitor_remote(public_key, tx).await,
        }
    }

    /// Monitor this local actor for state and status updates.
    ///
    /// Establishes monitoring for this local actor instance. This method is
    /// more efficient than the general `monitor` method when you know the
    /// actor is definitely local.
    ///
    /// # Arguments
    ///
    /// * `tx` - Channel to send updates to
    ///
    /// # Return
    ///
    /// `Result<(), MonitorError>` - Success or error during local observation setup
    ///
    /// # Errors
    ///
    /// Returns `MonitorError` if:
    /// - Actor is not found locally
    /// - Failed to send observation signal to actor
    #[cfg(feature = "monitor")]
    pub fn monitor_local(&self, tx: UpdateTx<A>) -> Result<(), MonitorError> {
        crate::monitor::monitor_local_id(self.id(), tx)
    }

    /// Monitor this remote actor for state and status updates.
    ///
    /// Establishes monitoring for this remote actor instance when you have
    /// the public key of the peer hosting it. This method bypasses local
    /// lookup and directly connects to the specified remote peer.
    ///
    /// # Arguments
    ///
    /// * `public_key` - Public key of the remote peer hosting the actor
    /// * `tx` - Channel to send updates to
    ///
    /// # Return
    ///
    /// `Result<(), RemoteError>` - Success or error during remote observation setup
    ///
    /// # Errors
    ///
    /// Returns `RemoteError` if:
    /// - Incorrect public key provided
    /// - Connection to remote peer fails
    /// - Remote actor is not found on the specified peer
    /// - Network communication errors occur
    #[cfg(all(feature = "monitor", feature = "remote"))]
    pub async fn monitor_remote(
        &self,
        public_key: PublicKey,
        tx: UpdateTx<A>,
    ) -> Result<(), RemoteError> {
        crate::monitor::monitor_remote_id(self.id(), public_key, tx).await
    }

    /// Look up an actor by identifier or URL.
    ///
    /// Determines lookup type by URL parsing - if successful performs remote lookup,
    /// otherwise performs local lookup. May establish network connection for URLs.
    #[cfg(feature = "remote")]
    pub async fn lookup(ident_or_url: impl AsRef<str>) -> Result<ActorRef<A>, RemoteError> {
        let ident_or_url = ident_or_url.as_ref();
        match Url::parse(ident_or_url) {
            Err(_) => Ok(Self::lookup_local(ident_or_url)?),
            Ok(url) => {
                let (ident, public_key) = parse_url(&url)?;
                Ok(Self::lookup_remote_impl(ident, public_key).await??)
            }
        }
    }

    /// Look up an actor on a specific remote node.
    ///
    /// # Arguments
    ///
    /// * `ident` - The identifier of the actor on the remote node
    /// * `public_key` - The public key of the target remote node
    ///
    /// # Return
    ///
    /// - `Ok(Ok(ActorRef<A>))` if the remote actor is found and accessible
    /// - `Ok(Err(LookupError))` if the actor is not found on the remote node
    /// - `Err(RemoteError)` if network communication fails
    ///
    /// # Network Effects
    ///
    /// Establishes network connection to remote peer if not already connected.
    #[cfg(feature = "remote")]
    pub async fn lookup_remote(
        ident: impl AsRef<str>,
        public_key: PublicKey,
    ) -> Result<Result<ActorRef<A>, BindingError>, RemoteError> {
        let ident = parse_ident(ident.as_ref())?;

        Self::lookup_remote_impl(ident, public_key).await
    }

    /// Look up an actor in the local actor registry.
    ///
    /// # Arguments
    ///
    /// * `ident` - The identifier to look up (name or UUID bytes)
    ///
    /// # Return
    ///
    /// `Result<ActorRef<A>, LookupError>` - The actor reference or lookup error.
    ///
    /// # Errors
    ///
    /// Returns `LookupError` if:
    /// - Actor not found by the given identifier
    /// - Type mismatch between expected and actual actor type
    pub fn lookup_local(ident: impl AsRef<str>) -> Result<ActorRef<A>, BindingError> {
        let ident = parse_ident(ident.as_ref())?;

        Self::lookup_local_impl(&ident)
    }

    #[cfg(feature = "remote")]
    pub(crate) async fn lookup_remote_impl(
        ident: Ident,
        public_key: PublicKey,
    ) -> Result<Result<ActorRef<A>, BindingError>, RemoteError> {
        // todo Flatten error
        let peer = LocalPeer::inst().get_or_connect_peer(public_key);

        peer.lookup(ident).await
    }

    pub(crate) fn lookup_local_impl(ident: &Ident) -> Result<ActorRef<A>, BindingError> {
        let entry = BINDINGS.get(ident).ok_or(BindingError::NotFound)?;

        let Some(any_actor) = entry.as_any() else {
            return Err(BindingError::NotFound);
        };

        let actor = any_actor
            .downcast_ref::<ActorRef<A>>()
            .ok_or(BindingError::DowncastError)?;

        Ok(actor.clone())
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

impl<A> Display for ActorRef<A>
where
    A: Actor,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", type_name::<A>(), Hex(self.0.id().as_bytes()))
    }
}

impl<A: Actor + Any> AnyActorRef for ActorRef<A> {
    fn id(&self) -> Option<ActorId> {
        Some(self.0.id())
    }

    #[cfg(feature = "remote")]
    fn send_tagged_bytes(&self, tag: Tag, bytes: Vec<u8>) -> Result<(), BytesSendError> {
        trace!(
            tag,
            bytes = bytes.len(),
            target = %self,
            "sending tagged bytes",
        );

        let msg = <A::Msg as FromTaggedBytes>::from(tag, &bytes)?;

        self.send(msg, Continuation::Nil)
            .map_err(|_| SendError((tag, bytes)))?;

        Ok(())
    }

    fn as_any(&self) -> Option<Box<dyn Any>> {
        Some(Box::new(self.clone()))
    }

    #[cfg(feature = "remote")]
    fn serialize(&self) -> Result<Vec<u8>, BindingError> {
        Ok(postcard::to_stdvec(&self)?)
    }

    #[cfg(feature = "remote")]
    fn spawn_export_task(&self, peer: Peer, mut rx_stream: RxStream) -> Result<(), BindingError> {
        tokio::spawn({
            let this = self.clone();

            PEER.scope(peer, async move {
                trace!(
                    actor = %this,
                    "listening exported",
                );

                loop {
                    let mut buf = Vec::new();
                    if let Err(err) = rx_stream.recv_frame_into(&mut buf).await {
                        return warn!(
                            actor = %this,
                            %err,
                            "stopped exported",
                        );
                    }

                    #[cfg(feature = "verbose")]
                    debug!(
                        to = %this,
                        len = buf.len(),
                        "received remote message bytes",
                    );

                    let (msg, k_dto) = match postcard::from_bytes::<MsgPackDto<A>>(&buf) {
                        Err(err) => {
                            error!(%err, "failed to deserialize remote message");
                            continue;
                        }
                        Ok(msg_k_dto) => msg_k_dto,
                    };

                    let (msg, k): MsgPack<A> = (msg, k_dto.into());
                    #[cfg(feature = "verbose")]
                    debug!(
                        to = %this,
                        msg = ?msg,
                        k = ?k,
                        "deserialized remote message",
                    );

                    if let Err(err) = this.send(msg, k) {
                        break warn!(actor = %this, %err, "stopped exported");
                    }
                }

                // todo Check if this is the last export task and if it is, the actor should be stopped
                // Now the global anon binding does not prevent drop, but still need to be cleared as it might cause leak of resources
                // How can we tell if it is the last export task
                // Or it might be just cleared when the actor is dropped
            })
        });

        Ok(())
    }

    #[cfg(all(feature = "remote", feature = "monitor"))]
    fn monitor_as_bytes(
        &self,
        peer: Peer,
        hdl: ActorHdl,
        mut tx_stream: TxStream,
    ) -> Result<(), MonitorError> {
        let (tx, rx) = unbounded_anonymous::<Update<A>>();

        if STATIC_MAX_LEVEL >= tracing::Level::WARN {
            // logging
            let (remote, actor) = (format!("{peer}"), format!("{self}"));

            tokio::spawn(PEER.scope(peer, async move {
                trace!(
                    %actor,
                    %remote,
                    "remote monitoring local",
                );

                loop {
                    let Some(update) = rx.recv().await else {
                        return warn!(
                            %actor,
                            %remote,
                            "remote monitoring channel closed"
                        );
                    };

                    let bytes = match postcard::to_stdvec(&update) {
                        Err(err) => {
                            warn!(
                                %actor,
                                %remote,
                                %err,
                                "failed to serialize update"
                            );
                            continue;
                        }
                        Ok(buf) => buf,
                    };

                    if let Err(err) = tx_stream.send_frame(&bytes).await {
                        break warn!(
                            %actor,
                            %remote,
                            %err,
                            "failed to send serialized update"
                        );
                    }
                }
            }));
        } else {
            // no logging
            tokio::spawn(PEER.scope(peer, async move {
                loop {
                    let Some(update) = rx.recv().await else {
                        return;
                    };

                    let bytes = match postcard::to_stdvec(&update) {
                        Err(_) => continue,
                        Ok(buf) => buf,
                    };

                    if tx_stream.send_frame(&bytes).await.is_err() {
                        break;
                    }
                }
            }));
        };

        hdl.monitor(Box::new(tx))
            .map_err(|_| MonitorError::SigSendError)?;

        Ok(())
    }

    #[cfg(feature = "remote")]
    fn ty_id(&self) -> ActorTypeId {
        A::IMPL_ID
    }

    #[cfg(feature = "remote")]
    fn sender_count(&self) -> usize {
        self.0.sender_count()
    }
}

impl<A> WeakActorRef<A>
where
    A: Actor,
{
    /// Get the unique identifier of the actor this weak reference points to.
    ///     
    /// # Return
    ///
    /// An `Option<ActorId>` which is None if the actor is dropped.
    pub fn ref_id(&self) -> usize {
        self.0.ptr_id()
    }

    /// Attempt to upgrade this weak reference to a strong reference.
    ///
    /// # Return
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

impl<A> AnyActorRef for WeakActorRef<A>
where
    A: Actor + Any,
{
    fn id(&self) -> Option<ActorId> {
        self.upgrade().map(|a| a.id())
    }

    #[cfg(feature = "remote")]
    fn send_tagged_bytes(&self, tag: Tag, bytes: Vec<u8>) -> Result<(), BytesSendError> {
        match self.upgrade() {
            None => Err(BytesSendError::SendError(SendError((tag, bytes)))),
            Some(actor) => actor.send_tagged_bytes(tag, bytes),
        }
    }

    fn as_any(&self) -> Option<Box<dyn Any>> {
        match self.upgrade() {
            None => None,
            Some(actor) => actor.as_any(),
        }
    }

    #[cfg(feature = "remote")]
    fn serialize(&self) -> Result<Vec<u8>, BindingError> {
        match self.upgrade() {
            None => Err(BindingError::NotFound),
            Some(actor) => actor.serialize(),
        }
    }

    #[cfg(feature = "remote")]
    fn spawn_export_task(&self, peer: Peer, rx_stream: RxStream) -> Result<(), BindingError> {
        match self.upgrade() {
            None => Err(BindingError::NotFound),
            Some(actor) => actor.spawn_export_task(peer, rx_stream),
        }
    }

    #[cfg(all(feature = "remote", feature = "monitor"))]
    fn monitor_as_bytes(
        &self,
        peer: Peer,
        hdl: ActorHdl,
        tx_stream: TxStream,
    ) -> Result<(), MonitorError> {
        match self.upgrade() {
            None => Err(MonitorError::SigSendError),
            Some(actor) => actor.monitor_as_bytes(peer, hdl, tx_stream),
        }
    }

    #[cfg(feature = "remote")]
    fn ty_id(&self) -> ActorTypeId {
        A::IMPL_ID
    }

    #[cfg(feature = "remote")]
    fn sender_count(&self) -> usize {
        match self.upgrade() {
            None => 0,
            Some(actor) => actor.sender_count(),
        }
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
    pub(crate) fn monitor(&self, tx: AnyUpdateTx) -> Result<(), SendError<RawSignal>> {
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
    /// # Return
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
                Err(_ret) => {
                    #[cfg(not(feature = "remote"))]
                    return Err(RequestError::DowncastError);
                    #[cfg(feature = "remote")]
                    {
                        let remote_reply_rx =
                            match _ret.downcast::<oneshot::Receiver<(Peer, Vec<u8>)>>() {
                                Err(_) => return Err(RequestError::DowncastError),
                                Ok(rx) => *rx,
                            };

                        let (peer, bytes) = remote_reply_rx.await?;

                        let res =
                            PEER.sync_scope(peer, || postcard::from_bytes::<M::Return>(&bytes))?;

                        Ok(res)
                    }
                }
                Ok(res) => Ok(*res), // Local reply
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
    /// # Return
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

// impl<A: Actor> ExportedActorRef<A> {
//     pub(crate) fn new(actor: WeakActorRef<A>, actor_id: ActorId) -> Self {
//         ExportedActorRef {
//             inner: Arc::new(ExportedActorRefInner { actor, actor_id }),
//         }
//     }
// }

// #[cfg(feature = "remote")]
// impl<A: Actor> Drop for ExportedActorRefInner<A> {
//     fn drop(&mut self) {
//         if RootContext::free_impl(self.actor_id.as_bytes()).is_none() {
//             error!(actor_id = %self.actor_id, "failed to free exported actor");
//         }
//     }
// }

// #[cfg(feature = "remote")]
// impl<A: Actor> AnyActorRef for ExportedActorRef<A> {
//     fn id(&self) -> ActorId {
//         self.inner.actor_id.into()
//     }

//     #[cfg(feature = "remote")]
//     fn send_tagged_bytes(&self, tag: Tag, bytes: Vec<u8>) -> Result<(), BytesSendError> {
//         match self.inner.actor.upgrade() {
//             None => Err(BytesSendError::SendError(SendError((tag, bytes)))),
//             Some(actor) => actor.send_tagged_bytes(tag, bytes),
//         }
//     }

//     fn as_any(&self) -> Option<Box<dyn Any>> {
//         match self.inner.actor.upgrade() {
//             None => None,
//             Some(actor) => actor.as_any(),
//         }
//     }

//     #[cfg(feature = "remote")]
//     fn serialize(&self) -> Result<Vec<u8>, LookupError> {
//         match self.inner.actor.upgrade() {
//             None => Err(LookupError::NotFound),
//             Some(actor) => actor.serialize(),
//         }
//     }

//     #[cfg(feature = "remote")]
//     fn spawn_export_task(&self, peer: Peer, rx_stream: RxStream) -> Result<(), LookupError> {
//         match self.inner.actor.upgrade() {
//             None => Err(LookupError::NotFound),
//             Some(actor) => actor.spawn_export_task(peer, rx_stream),
//         }
//     }

//     #[cfg(all(feature = "remote", feature = "monitor"))]
//     fn monitor_as_bytes(
//         &self,
//         peer: Peer,
//         hdl: ActorHdl,
//         tx_stream: TxStream,
//     ) -> Result<(), MonitorError> {
//         match self.inner.actor.upgrade() {
//             None => Err(MonitorError::SigSendError),
//             Some(actor) => actor.monitor_as_bytes(peer, hdl, tx_stream),
//         }
//     }

//     #[cfg(feature = "remote")]
//     fn ty_id(&self) -> ActorTypeId {
//         A::IMPL_ID
//     }

//     #[cfg(feature = "remote")]
//     fn sender_count(&self) -> usize {
//         match self.inner.actor.upgrade() {
//             None => 0,
//             Some(actor) => actor.sender_count(),
//         }
//     }
// }
