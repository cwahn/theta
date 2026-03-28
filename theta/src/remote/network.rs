use std::sync::Arc;

use bytes::Bytes;
use futures::{
    FutureExt,
    future::{BoxFuture, Shared},
    lock::Mutex,
};
use iroh::{
    Endpoint, EndpointAddr, PublicKey,
    endpoint::{Connection, RecvStream, SendStream},
};

use thiserror::Error;

/// Errors that can occur during network operations.
#[derive(Debug, Clone, Error)]
pub enum NetworkError {
    #[error(transparent)]
    ConnectError(#[from] Arc<iroh::endpoint::ConnectError>),
    #[error(transparent)]
    ConnectionError(#[from] Arc<iroh::endpoint::ConnectionError>),
    #[error(transparent)]
    ConnectingError(#[from] Arc<iroh::endpoint::ConnectingError>),
    #[error("peer closed while accepting")]
    PeerClosedWhileAccepting,
    #[error(transparent)]
    IoError(#[from] Arc<std::io::Error>),
    #[error(transparent)]
    ReadExactError(#[from] Arc<iroh::endpoint::ReadExactError>),
    #[error(transparent)]
    WriteError(#[from] Arc<iroh::endpoint::WriteError>),
}

// Extension traits — zero-cost async fn in traits (edition 2024)

pub(crate) trait SendFrameExt {
    #[allow(dead_code)]
    async fn send_frame(&mut self, data: Vec<u8>) -> Result<(), NetworkError>;
}

pub(crate) trait RecvFrameExt {
    async fn recv_frame_into(&mut self, buf: &mut Vec<u8>) -> Result<(), NetworkError>;
}

impl SendFrameExt for SendStream {
    #[inline]
    async fn send_frame(&mut self, data: Vec<u8>) -> Result<(), NetworkError> {
        #[cfg(feature = "perf-instrument")]
        let t_len = std::time::Instant::now();
        let len_bytes = Bytes::copy_from_slice(&(data.len() as u32).to_be_bytes());
        let data_bytes = Bytes::from(data);
        #[cfg(feature = "perf-instrument")]
        crate::perf_instrument::WRITE_LEN_PREFIX_NS.fetch_add(
            t_len.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        #[cfg(feature = "perf-instrument")]
        let t_data = std::time::Instant::now();
        self.write_all_chunks(&mut [len_bytes, data_bytes])
            .await
            .map_err(|e| NetworkError::WriteError(Arc::new(e)))?;
        #[cfg(feature = "perf-instrument")]
        {
            crate::perf_instrument::WRITE_DATA_NS.fetch_add(
                t_data.elapsed().as_nanos() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            crate::perf_instrument::WRITE_FRAME_COUNT
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }

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

impl Network {
    pub(crate) fn new(endpoint: iroh::Endpoint) -> Self {
        Self { endpoint }
    }

    pub(crate) fn public_key(&self) -> iroh::PublicKey {
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
            // Use cached addresses from a prior connection if available.
            // Otherwise, provide our relay URL as a routing hint: iroh will
            // connect through relay immediately, then QNT upgrades to direct
            // UDP in the background. This avoids depending on pkarr DNS
            // propagation timing for the initial connection.
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
        }
        .boxed()
        .shared();

        PreparedConn { inner: fut }
    }

    /// Constructs an [`EndpointAddr`] with our relay URL as a routing hint.
    /// iroh will connect through relay immediately, then QNT upgrades to
    /// direct UDP once holepunching succeeds.
    /// Falls back to pubkey-only addressing if relay isn't ready within 5 s.
    async fn addr_with_relay_fallback(endpoint: &Endpoint, public_key: PublicKey) -> EndpointAddr {
        // endpoint.online() blocks until the home relay is established.
        // If relays are unreachable, this would hang forever — so we bound it.
        if crate::compat::timeout(std::time::Duration::from_secs(5), endpoint.online())
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

        let inner = async move {
            Ok(PreparedConnInner {
                conn,
                control_tx: Arc::new(Mutex::new(control_tx)),
            })
        }
        .boxed()
        .shared();

        Ok((public_key, PreparedConn { inner }))
    }
}

// This is what will be actually used

#[derive(Debug, Clone)]
pub(crate) struct PreparedConn {
    inner: Shared<BoxFuture<'static, Result<PreparedConnInner, NetworkError>>>,
}

#[derive(Debug, Clone)]
struct PreparedConnInner {
    conn: Connection,
    control_tx: Arc<Mutex<SendStream>>,
}

impl PreparedConn {
    pub(crate) async fn send_control_frame(&self, data: &[u8]) -> Result<(), NetworkError> {
        let inner = self.get().await?;

        #[cfg(feature = "perf-instrument")]
        let t_mutex = std::time::Instant::now();
        let mut guard = inner.control_tx.lock().await;
        #[cfg(feature = "perf-instrument")]
        crate::perf_instrument::CTRL_MUTEX_WAIT_NS.fetch_add(
            t_mutex.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        #[cfg(feature = "perf-instrument")]
        let t_write = std::time::Instant::now();
        let result = guard.send_frame(data.to_vec()).await;
        #[cfg(feature = "perf-instrument")]
        crate::perf_instrument::CTRL_WRITE_NS.fetch_add(
            t_write.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        result
    }

    // ! Should be called only once
    pub(crate) async fn control_rx(&self) -> Result<RecvStream, NetworkError> {
        let inner = self.get().await?;

        inner
            .conn
            .accept_uni()
            .await
            .map_err(|e| NetworkError::ConnectionError(Arc::new(e)))
    }

    pub(crate) async fn open_bi(&self) -> Result<(SendStream, RecvStream), NetworkError> {
        #[cfg(feature = "perf-instrument")]
        let t = std::time::Instant::now();
        let inner = self.get().await?;

        let result = inner
            .conn
            .open_bi()
            .await
            .map_err(|e| NetworkError::ConnectionError(Arc::new(e)));
        #[cfg(feature = "perf-instrument")]
        {
            crate::perf_instrument::OPEN_BI_NS.fetch_add(
                t.elapsed().as_nanos() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            crate::perf_instrument::OPEN_BI_COUNT
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        result
    }

    pub(crate) async fn accept_bi(&self) -> Result<(SendStream, RecvStream), NetworkError> {
        #[cfg(feature = "perf-instrument")]
        let t = std::time::Instant::now();
        let inner = self.get().await?;

        let result = inner
            .conn
            .accept_bi()
            .await
            .map_err(|e| NetworkError::ConnectionError(Arc::new(e)));
        #[cfg(feature = "perf-instrument")]
        {
            crate::perf_instrument::ACCEPT_BI_NS.fetch_add(
                t.elapsed().as_nanos() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            crate::perf_instrument::ACCEPT_BI_COUNT
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        result
    }

    pub(crate) async fn open_uni(&self) -> Result<SendStream, NetworkError> {
        let inner = self.get().await?;

        inner
            .conn
            .open_uni()
            .await
            .map_err(|e| NetworkError::ConnectionError(Arc::new(e)))
    }

    pub(crate) async fn accept_uni(&self) -> Result<RecvStream, NetworkError> {
        let inner = self.get().await?;

        inner
            .conn
            .accept_uni()
            .await
            .map_err(|e| NetworkError::ConnectionError(Arc::new(e)))
    }

    async fn get(&self) -> Result<PreparedConnInner, NetworkError> {
        #[cfg(feature = "perf-instrument")]
        let t = std::time::Instant::now();
        let result = self.inner.clone().await;
        #[cfg(feature = "perf-instrument")]
        {
            crate::perf_instrument::CONN_GET_NS.fetch_add(
                t.elapsed().as_nanos() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            crate::perf_instrument::CONN_GET_COUNT
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        result
    }
}
