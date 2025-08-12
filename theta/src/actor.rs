use std::{fmt::Debug, future::Future, panic::UnwindSafe};

use crate::{
    context::Context,
    errors::ExitCode,
    message::{Continuation, Escalation, Signal},
};

#[cfg(feature = "remote")]
use {
    crate::{
        actor_ref::ActorRef,
        base::AnyActorRef,
        error,
        remote::{
            base::{ActorImplId, ReplyKey, Tag},
            peer::{CURRENT_PEER, RemotePeer},
            serde::{FromTaggedBytes, MsgPackDto},
        },
        warn,
    },
    futures::future::BoxFuture,
    serde::{Deserialize, Serialize},
    std::any::type_name,
    theta_protocol::core::Receiver,
};

pub type ActorId = uuid::Uuid;

// ! Currently, the restart sementic should be considered ill-designed.
// todo Redesign resilience system.

pub trait ActorArgs: Clone + Send + UnwindSafe + 'static {
    type Actor: Actor;

    /// An initialization logic of an actor.
    /// - Panic-safe; panic will get caught and escalated
    fn initialize(
        ctx: Context<Self::Actor>,
        args: &Self,
    ) -> impl Future<Output = Self::Actor> + Send + UnwindSafe;
}

pub trait Actor: Sized + Debug + Send + UnwindSafe + 'static {
    #[cfg(not(feature = "remote"))]
    type Msg: Send;
    #[cfg(feature = "remote")]
    type Msg: Send + Serialize + for<'de> Deserialize<'de> + FromTaggedBytes;

    /// A type used for monitoring the actor state.
    #[cfg(not(feature = "remote"))]
    type StateReport: Send + UnwindSafe + Clone + for<'a> From<&'a Self> + 'static;

    #[cfg(feature = "remote")]
    type StateReport: Send
        + UnwindSafe
        + Clone
        + for<'a> From<&'a Self>
        + Serialize
        + for<'de> Deserialize<'de>;

    /// A wrapper around message processing for optional monitoring.
    /// - Panic-safe; panic will get caught and escalated
    /// - Could be manually implemented, but recommended to use ['intention!'] macro
    #[allow(unused_variables)]
    fn process_msg(
        &mut self,
        ctx: Context<Self>,
        msg: Self::Msg,
        k: Continuation,
    ) -> impl Future<Output = ()> + Send;

    /// Handles escalation from children
    /// - Panic-safe; panic will get caught and escalated
    /// - It is recommended to set max_restart and in_period to prevent infinite loop
    #[allow(unused_variables)]
    fn supervise(
        &mut self,
        escalation: Escalation,
    ) -> impl Future<Output = (Signal, Option<Signal>)> + Send {
        __default_supervise(self, escalation)
    }

    /// Called on on restart, before initialization
    /// - Panic-safe; but the panic will not be escalated but ignored or logged
    /// - State might be corrupted since it does not rollback on panic
    #[allow(unused_variables)]
    fn on_restart(&mut self) -> impl Future<Output = ()> + Send {
        __default_on_restart(self)
    }

    /// Called on drop, or termination
    /// - Panic-safe; but the panic will not be escalated but ignored or logged
    /// - In case of termination, state might be corrupted since it does not rollback on panic
    /// - Since the message loop is already stopped, any message to self will be lost
    #[allow(unused_variables)]
    fn on_exit(&mut self, exit_code: ExitCode) -> impl Future<Output = ()> + Send {
        __default_on_exit(self, exit_code)
    }

    #[allow(unused_variables)]
    fn hash_code(&self) -> u64 {
        0 // no-op by default
    }

    #[allow(unused_variables)]
    fn state_report(&self) -> Self::StateReport {
        self.into() // no-op by default
    }

    /// Should not implemented by user.
    #[cfg(feature = "remote")]
    const IMPL_ID: ActorImplId;

    #[cfg(feature = "remote")]
    fn export_task_fn(
        remote_peer: RemotePeer,
        in_stream: Box<dyn Receiver>,
        actor: AnyActorRef,
    ) -> BoxFuture<'static, ()> {
        Box::pin(CURRENT_PEER.scope(remote_peer, async move {
            let Some(actor) = actor.downcast_ref::<ActorRef<Self>>() else {
                return error!(
                    "Failed to downcast any actor reference to {}",
                    type_name::<Self>()
                );
            };

            loop {
                let Ok(bytes) = in_stream.recv_datagram().await else {
                    break error!("Failed to receive datagram from stream");
                };

                let Ok(msg_k_dto) = postcard::from_bytes::<MsgPackDto<Self>>(&bytes) else {
                    warn!("Failed to deserialize msg pack dto");
                    continue;
                };

                let (msg, k) = msg_k_dto.into();

                if let Err(e) = actor.send_raw(msg, k) {
                    break error!("Failed to send message to actor: {e}");
                }
            }
        }))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nil;

// Delegated default implementation in order to decouple it from macro expansion

pub fn __default_supervise<A: Actor>(
    _actor: &mut A,
    _escalation: Escalation,
) -> impl std::future::Future<Output = (Signal, Option<Signal>)> + Send {
    async move { (Signal::Terminate, None) }
}

pub fn __default_on_restart<A: Actor>(
    _actor: &mut A,
) -> impl std::future::Future<Output = ()> + Send {
    async move {}
}

pub fn __default_on_exit<A: Actor>(
    _actor: &mut A,
    _exit_code: ExitCode,
) -> impl std::future::Future<Output = ()> + Send {
    async move {}
}

// Implementations

impl<T> From<&T> for Nil
where
    T: Actor,
{
    fn from(_: &T) -> Self {
        Nil
    }
}
