pub mod codec;
pub mod peer;
pub mod registry;
pub mod serde;

// Re-exports

use std::{any::Any, fmt::Debug};

use futures::{FutureExt, future::BoxFuture};
pub use registry::{
    ACTOR_REGISTRY, ActorEntry, ActorRegistry, BehaviorEntry, BehaviorRegistry, RegisterActorFn,
    RegisterBehaviorFn,
};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, WeakUnboundedSender};
use uuid::Uuid;

use crate::{
    actor::Actor,
    context::Context,
    message::{Behavior, Continuation, Message},
};

pub type Tid = Uuid;
pub type ActorTid = Tid;
pub type MsgTid = Tid;

pub type BoxedRemoteMsg<A> = Box<dyn RemoteMessage<A>>;

pub type RemoteMsgPack<A> = (BoxedRemoteMsg<A>, Continuation);

pub type RemoteMsgTx<A> = UnboundedSender<RemoteMsgPack<A>>;
pub type WeakRemoteMsgTx<A> = WeakUnboundedSender<RemoteMsgPack<A>>;
pub type RemoteMsgRx<A> = UnboundedReceiver<RemoteMsgPack<A>>;

// todo #[tid = "Uuid"] macro
pub trait Remote {
    const TID: Tid;
}

pub trait RemoteMessage<A>: Message<A> + erased_serde::Serialize
where
    A: Actor,
{
    fn tid(&self) -> MsgTid;
}

// Implementations

impl<A, M> RemoteMessage<A> for M
where
    A: Actor + Behavior<M>,
    M: Message<A> + Remote + erased_serde::Serialize + 'static,
{
    fn tid(&self) -> MsgTid {
        M::TID
    }
}
