use std::sync::Arc;

use futures::future::BoxFuture;
use theta_protocol::core::Receiver;
use uuid::Uuid;

use crate::{actor_ref::AnyActorRef, remote::peer::RemotePeer};

pub type ImplId = Uuid;
pub type ActorImplId = ImplId;

pub(crate) type ReplyKey = Uuid;

pub(crate) type Tag = u32;

pub(crate) type ExportTaskFn =
    fn(RemotePeer, Box<dyn Receiver>, Arc<dyn AnyActorRef>) -> BoxFuture<'static, ()>;
