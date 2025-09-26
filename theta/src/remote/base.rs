use futures::channel::oneshot::Canceled;
use iroh::PublicKey;
use thiserror::Error;
use tokio::time::error::Elapsed;
use uuid::Uuid;

use crate::{
    base::{BindingError, Ident, MonitorError, parse_ident},
    remote::network::NetworkError,
};

/// Unique identifier for actor implementation types in remote communication.
pub type ActorTypeId = Uuid;

/// Message type identifier for remote serialization.
pub type Tag = u32;

pub(crate) type Key = u32;

/// Errors that can occur during remote actor operations.
#[derive(Debug, Clone, Error)]
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
    BindingError(#[from] BindingError),
    #[error(transparent)]
    MonitorError(#[from] MonitorError),

    #[error("deadline has elapsed")]
    Timeout,
}

// #[derive(Debug, Clone)]
// pub(crate) struct Cancel {
//     inner: Arc<Inner>,
// }

// #[derive(Debug)]
// struct Inner {
//     notify: Notify,
//     canceled: AtomicBool,
// }

/// Parse IROH URL into identifier and public key components.
///
/// Supports both UUID and string identifiers in the format:
/// `iroh://{ident}@{public_key}`
pub(crate) fn split_url(addr: &url::Url) -> Result<(Ident, PublicKey), RemoteError> {
    let ident = parse_ident(addr.username())?;

    let public_key = addr
        .host_str()
        .ok_or(RemoteError::InvalidAddress)?
        .parse::<PublicKey>()
        .map_err(|_| RemoteError::InvalidAddress)?;

    debug_assert!(addr.path_segments().is_none());

    Ok((ident, public_key))
}

// Implementation

// impl Cancel {
//     #[inline]
//     pub fn new() -> Self {
//         Self {
//             inner: Arc::new(Inner {
//                 notify: Notify::new(),
//                 canceled: AtomicBool::new(false),
//             }),
//         }
//     }

//     #[inline]
//     pub fn is_canceled(&self) -> bool {
//         self.inner.canceled.load(Ordering::Relaxed)
//     }

//     /// Cancel and return previous cancel state which should be false.
//     #[cold]
//     pub fn cancel(&self) -> bool {
//         match self.inner.canceled.swap(true, Ordering::Release) {
//             false => {
//                 self.inner.notify.notify_waiters();
//                 false
//             }
//             true => true,
//         }
//     }

//     pub async fn canceled(&self) {
//         if self.inner.canceled.load(Ordering::Relaxed) {
//             return;
//         }
//         let notified = self.inner.notify.notified();
//         if self.inner.canceled.load(Ordering::Acquire) {
//             return;
//         }
//         notified.await;
//     }
// }

// impl Default for Cancel {
//     fn default() -> Self {
//         Self::new()
//     }
// }

impl From<Elapsed> for RemoteError {
    fn from(_: Elapsed) -> Self {
        RemoteError::Timeout
    }
}

#[cfg(test)]
mod tests {
    use url::Url;
    use uuid::uuid;

    use super::*;

    #[test]
    fn test_split_url() {
        let url = Url::parse("iroh://824d7cba-1489-4537-b2c9-1a488a3f895a@a0f71647936e25b8403433b31deb3a374d175b282baf9803a7715b138f9e6f65").unwrap();
        let (ident, public_key) = split_url(&url).unwrap();
        assert_eq!(
            public_key.to_string(),
            "a0f71647936e25b8403433b31deb3a374d175b282baf9803a7715b138f9e6f65"
        );
        assert_eq!(
            &ident,
            uuid!("824d7cba-1489-4537-b2c9-1a488a3f895a").as_bytes()
        );

        let url = Url::parse(
            "iroh://foo@a0f71647936e25b8403433b31deb3a374d175b282baf9803a7715b138f9e6f65",
        )
        .unwrap();
        let (ident, public_key) = split_url(&url).unwrap();
        assert_eq!(
            public_key.to_string(),
            "a0f71647936e25b8403433b31deb3a374d175b282baf9803a7715b138f9e6f65"
        );
        let expected: [u8; 16] = {
            let mut bytes = [0u8; 16];
            bytes[..3].copy_from_slice(b"foo");
            bytes
        };
        assert_eq!(&ident, &expected);
    }
}
