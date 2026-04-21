use std::sync::Arc;

use bytes::Bytes;
use futures::{
    FutureExt,
    future::{BoxFuture, Shared},
    lock::Mutex,
};
use iroh::{
    Endpoint, EndpointAddr, PublicKey,
    endpoint::ConnectError,
    endpoint::ConnectingError,
    endpoint::ConnectionError,
    endpoint::ReadExactError,
    endpoint::WriteError,
    endpoint::{Connection, RecvStream, SendStream},
};
use thiserror::Error;

use crate::compat;

pub(crate) trait SendFrameExt {
    #[allow(dead_code)]
    async fn send_frame(&mut self, data: Vec<u8>) -> Result<(), NetworkError>;
}

pub(crate) trait RecvFrameExt {
    async fn recv_frame_into(&mut self, buf: &mut Vec<u8>) -> Result<(), NetworkError>;
}

/// Errors that can occur during network operations.
#[derive(Debug, Clone, Error)]
pub enum NetworkError {
    #[error(transparent)]
    ConnectError(#[from] Arc<ConnectError>),
    #[error(transparent)]
    ConnectionError(#[from] Arc<ConnectionError>),
    #[error(transparent)]
    ConnectingError(#[from] Arc<ConnectingError>),
    #[error("peer closed while accepting")]
    PeerClosedWhileAccepting,
    #[error(transparent)]
    IoError(#[from] Arc<std::io::Error>),
    #[error(transparent)]
    ReadExactError(#[from] Arc<ReadExactError>),
    #[error(transparent)]
    WriteError(#[from] Arc<WriteError>),
}

impl SendFrameExt for SendStream {
    #[inline]
    async fn send_frame(&mut self, data: Vec<u8>) -> Result<(), NetworkError> {
        let len_bytes = Bytes::copy_from_slice(&(data.len() as u32).to_be_bytes());
        let data_bytes = Bytes::from(data);

        self.write_all_chunks(&mut [len_bytes, data_bytes])
            .await
            .map_err(|e| NetworkError::WriteError(Arc::new(e)))?;

        Ok(())
    }
}

impl RecvFrameExt for RecvStream {
    #[inline]
    async fn recv_frame_into(&mut self, buf: &mut Vec<u8>) -> Result<(), NetworkError> {
        let mut len_buf = [0u8; 4];

        self.read_exact(&mut len_buf)
            .await
            .map_err(|e| NetworkError::ReadExactError(Arc::new(e)))?;

        let len = u32::from_be_bytes(len_buf) as usize;

        buf.resize(len, 0);
        self.read_exact(buf)
            .await
            .map_err(|e| NetworkError::ReadExactError(Arc::new(e)))?;

        Ok(())
    }
}

/// IROH-based networking backend for remote actor communication.
#[derive(Debug, Clone)]
pub(crate) struct Network {
    pub(crate) endpoint: Endpoint,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedConn {
    inner: Shared<BoxFuture<'static, Result<PreparedConnInner, NetworkError>>>,
}

impl Network {
    pub(crate) fn new(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }

    pub(crate) fn public_key(&self) -> PublicKey {
        self.endpoint.id()
    }

    pub(crate) async fn connect(&self, addr: EndpointAddr) -> Result<Connection, NetworkError> {
        let conn = self
            .endpoint
            .connect(addr, b"theta")
            .await
            .map_err(|e| NetworkError::ConnectError(Arc::new(e)))?;

        Ok(conn)
    }

    pub(crate) async fn accept(&self) -> Result<(PublicKey, Connection), NetworkError> {
        let Some(incoming) = self.endpoint.accept().await else {
            return Err(NetworkError::PeerClosedWhileAccepting);
        };

        let accepting = incoming
            .accept()
            .map_err(|e| NetworkError::ConnectionError(Arc::new(e)))?;

        let conn = match accepting.await {
            Err(e) => return Err(NetworkError::ConnectingError(Arc::new(e))),
            Ok(conn) => conn,
        };

        let public_key = conn.remote_id();

        Ok((public_key, conn))
    }

    pub(crate) fn connect_and_prepare(&self, public_key: PublicKey) -> PreparedConn {
        let this = self.clone();
        let fut = async move {
            let addr = match this
                .endpoint
                .remote_info(public_key)
                .await
                .filter(|info| info.addrs().next().is_some())
            {
                Some(info) => {
                    let id = info.id();

                    EndpointAddr::from_parts(id, info.into_addrs().map(|a| a.into_addr()))
                }
                None => Self::addr_with_relay_fallback(&this.endpoint, public_key).await,
            };

            let conn = this.connect(addr).await?;
            let control_tx = conn
                .open_uni()
                .await
                .map_err(|e| NetworkError::ConnectionError(Arc::new(e)))?;

            Ok(PreparedConnInner {
                conn,
                control_tx: Arc::new(Mutex::new(control_tx)),
            })
        };

        PreparedConn {
            inner: fut.boxed().shared(),
        }
    }

    /// Constructs an [`EndpointAddr`] with our relay URL as a routing hint.
    /// iroh will connect through relay immediately, then QNT upgrades to
    /// direct UDP once holepunching succeeds.
    /// Falls back to pubkey-only addressing if relay isn't ready within 5 s.
    async fn addr_with_relay_fallback(endpoint: &Endpoint, public_key: PublicKey) -> EndpointAddr {
        if compat::timeout(std::time::Duration::from_secs(5), endpoint.online())
            .await
            .is_err()
        {
            tracing::warn!("relay not ready within 5 s, connecting without relay hint");

            return EndpointAddr::from(public_key);
        }

        let mut addrs = endpoint.addr().addrs;

        addrs.retain(|a| a.is_relay());

        if addrs.is_empty() {
            EndpointAddr::from(public_key)
        } else {
            EndpointAddr::from_parts(public_key, addrs)
        }
    }

    pub(crate) async fn accept_and_prepare(
        &self,
    ) -> Result<(PublicKey, PreparedConn), NetworkError> {
        let (public_key, conn) = self.accept().await?;
        let control_tx = conn
            .open_uni()
            .await
            .map_err(|e| NetworkError::ConnectionError(Arc::new(e)))?;
        let fut = async move {
            Ok(PreparedConnInner {
                conn,
                control_tx: Arc::new(Mutex::new(control_tx)),
            })
        };

        Ok((
            public_key,
            PreparedConn {
                inner: fut.boxed().shared(),
            },
        ))
    }
}

impl PreparedConn {
    pub(crate) async fn send_control_frame(&self, data: Vec<u8>) -> Result<(), NetworkError> {
        let inner = self.get().await?;

        let mut guard = inner.control_tx.lock().await;

        guard.send_frame(data).await
    }

    pub(crate) async fn control_rx(&self) -> Result<RecvStream, NetworkError> {
        let inner = self.get().await?;

        inner
            .conn
            .accept_uni()
            .await
            .map_err(|e| NetworkError::ConnectionError(Arc::new(e)))
    }

    pub(crate) async fn open_bi(&self) -> Result<(SendStream, RecvStream), NetworkError> {
        let inner = self.get().await?;

        inner
            .conn
            .open_bi()
            .await
            .map_err(|e| NetworkError::ConnectionError(Arc::new(e)))
    }

    pub(crate) async fn accept_bi(&self) -> Result<(SendStream, RecvStream), NetworkError> {
        let inner = self.get().await?;

        inner
            .conn
            .accept_bi()
            .await
            .map_err(|e| NetworkError::ConnectionError(Arc::new(e)))
    }

    /// Close the underlying QUIC connection (if established).
    /// Fires and forgets — spawns a task that resolves the inner future and closes.
    pub(crate) fn close(&self, reason: &'static [u8]) {
        let inner = self.inner.clone();

        compat::spawn(async move {
            if let Ok(inner) = inner.await {
                inner
                    .conn
                    .close(iroh::endpoint::VarInt::from_u32(0), reason);
            }
        });
    }

    async fn get(&self) -> Result<PreparedConnInner, NetworkError> {
        self.inner.clone().await
    }
}

#[derive(Debug, Clone)]
struct PreparedConnInner {
    conn: Connection,
    control_tx: Arc<Mutex<SendStream>>,
}
