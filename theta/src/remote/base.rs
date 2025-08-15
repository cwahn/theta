use futures::channel::oneshot::Canceled;
use iroh::PublicKey;
use thiserror::Error;
use tokio::time::error::Elapsed;
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
pub(crate) fn split_url(addr: &url::Url) -> Result<(Ident, PublicKey), RemoteError> {
    let ident = addr.username().as_bytes().to_vec().into();
    let public_key = addr
        .host_str()
        .ok_or(RemoteError::InvalidAddress)?
        .parse::<PublicKey>()
        .map_err(|_| RemoteError::InvalidAddress)?;

    debug_assert!(addr.path_segments().is_none());

    Ok((ident, public_key))
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::*;

    #[test]
    fn test_split_url() {
        let url = Url::parse("iroh://824d7cba-1489-4537-b2c9-1a488a3f895a@a0f71647936e25b8403433b31deb3a374d175b282baf9803a7715b138f9e6f65").unwrap();
        let (ident, public_key ) = split_url(&url).unwrap();
        assert_eq!(public_key.to_string(), "a0f71647936e25b8403433b31deb3a374d175b282baf9803a7715b138f9e6f65");
        assert_eq!(&*ident, b"824d7cba-1489-4537-b2c9-1a488a3f895a");

        let url = Url::parse("iroh://foo@a0f71647936e25b8403433b31deb3a374d175b282baf9803a7715b138f9e6f65").unwrap();
        let (ident, public_key) = split_url(&url).unwrap();
        assert_eq!(public_key.to_string(), "a0f71647936e25b8403433b31deb3a374d175b282baf9803a7715b138f9e6f65");
        assert_eq!(&*ident, b"foo");
    }
}
