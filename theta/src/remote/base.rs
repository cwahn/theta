use std::sync::Arc;

use futures::future::BoxFuture;
use theta_protocol::core::Receiver;
use thiserror::Error;
use uuid::Uuid;

use crate::{actor_ref::AnyActorRef, context::LookupError, remote::peer::RemotePeer};

pub type ImplId = Uuid;
pub type ActorImplId = ImplId;

pub(crate) type ReplyKey = Uuid;

pub(crate) type Tag = u32;

pub(crate) type ExportTaskFn =
    fn(RemotePeer, Box<dyn Receiver>, Arc<dyn AnyActorRef>) -> BoxFuture<'static, ()>;

#[derive(Debug, Error)]
pub enum RemoteError {
    #[error("Invalid address")]
    InvalidAddress,
    #[error(transparent)]
    NetworkError(#[from] theta_protocol::error::Error),

    #[error(transparent)]
    SerializeError(postcard::Error),
    #[error(transparent)]
    DeserializeError(postcard::Error),

    #[error(transparent)]
    LookupError(#[from] LookupError),
}
