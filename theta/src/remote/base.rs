use std::str::FromStr;

use futures::channel::oneshot::Canceled;
use iroh::PublicKey;
use thiserror::Error;
use tokio::time::error::Elapsed;
use url::Url;
use uuid::Uuid;

use crate::{
    base::Ident,
    context::{LookupError, ObserveError},
    remote::network::NetworkError,
};

pub type ActorTypeId = Uuid;
pub type Tag = u32;

pub(crate) type ReplyKey = u64;

#[derive(Debug, Error)]
pub enum RemoteError {
    #[error(transparent)]
    Canceled(#[from] Canceled),

    #[error("invalid address")]
    InvalidAddress,

    #[error(transparent)]
    NetworkError(#[from] NetworkError),

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

/// Split a URL into public key and identifier
pub(crate) fn split_url(addr: &Url) -> Result<(PublicKey, Ident), RemoteError> {
    // First segment is the public key
    // Second segment is the actor identifier
    let mut segments = addr.path_segments().ok_or(RemoteError::InvalidAddress)?;

    let public_key = segments
        .next()
        .and_then(|s| PublicKey::from_str(s).ok())
        .ok_or(RemoteError::InvalidAddress)?;

    let ident = segments
        .next()
        .ok_or(RemoteError::InvalidAddress)?
        .as_bytes()
        .to_vec()
        .into();

    // Should be empty
    debug_assert!(segments.next().is_none());

    Ok((public_key, ident))
}
