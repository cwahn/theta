use std::sync::Arc;

use futures::{channel::oneshot::Canceled, future::BoxFuture};
use theta_protocol::core::Receiver;
use thiserror::Error;
use tokio::time::error::Elapsed;
use uuid::Uuid;

use crate::{
    actor_ref::AnyActorRef,
    context::{LookupError, ObserveError},
    remote::peer::Peer,
};

pub type ImplId = Uuid;
pub type ActorTypeId = ImplId;

pub(crate) type ReplyKey = u64;

pub(crate) type Tag = u32;

pub(crate) type ExportTaskFn =
    fn(Peer, Box<dyn Receiver>, Arc<dyn AnyActorRef>) -> BoxFuture<'static, ()>;

#[derive(Debug, Error)]
pub enum RemoteError {
    #[error(transparent)]
    Canceled(#[from] Canceled),

    #[error("invalid address")]
    InvalidAddress,
    #[error(transparent)]
    NetworkError(#[from] theta_protocol::error::Error),

    #[error(transparent)]
    SerializeError(postcard::Error),
    #[error(transparent)]
    DeserializeError(postcard::Error),

    #[error(transparent)]
    LookupError(#[from] LookupError),
    #[error(transparent)]
    ObserveError(#[from] ObserveError),

    #[error(transparent)]
    Timeout(#[from] Elapsed),
}
