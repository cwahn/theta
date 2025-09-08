use std::sync::Arc;

use futures::{
    FutureExt,
    future::{BoxFuture, Shared},
    lock::Mutex,
};
use iroh::{
    Endpoint, NodeAddr, PublicKey,
    endpoint::{Connection, RecvStream, SendStream},
};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Errors that can occur during network operations.
#[derive(Debug, Clone, Error)]
pub enum NetworkError {
    #[error(transparent)]
    ConnectError(#[from] Arc<iroh::endpoint::ConnectError>),
    // #[error(transparent)]
    // SendDatagramError(#[from] Arc<iroh::endpoint::SendDatagramError>),
    #[error(transparent)]
    ConnectionError(#[from] Arc<iroh::endpoint::ConnectionError>),
    #[error("peer closed while accepting")]
    PeerClosedWhileAccepting,
    #[error(transparent)]
    IoError(#[from] Arc<std::io::Error>),
    #[error(transparent)]
    WriteError(#[from] Arc<iroh::endpoint::WriteError>),
    #[error(transparent)]
    ReadExactError(#[from] Arc<iroh::endpoint::ReadExactError>),
}

/// IROH-based networking backend for remote actor communication.
#[derive(Debug, Clone)]
pub(crate) struct Network {
    pub(crate) endpoint: Endpoint,
}

/// Transport layer for IROH network connections.
#[derive(Debug, Clone)]
pub(crate) struct Transport {
    conn: Connection,
}

// todo Make pub(crate) by separating AnyActorRef trait
/// Stream for sending data over IROH connections.
#[derive(Debug)]
pub struct TxStream(SendStream);

// todo Make pub(crate) by separating AnyActorRef trait
/// Stream for receiving data over IROH connections.
#[derive(Debug)]
pub struct RxStream(RecvStream);

// Implementation

impl Network {
    pub(crate) fn new(endpoint: iroh::Endpoint) -> Self {
        Self { endpoint }
    }

    pub(crate) fn public_key(&self) -> iroh::PublicKey {
        self.endpoint.node_id()
    }

    pub(crate) async fn connect(&self, addr: NodeAddr) -> Result<Transport, NetworkError> {
        let conn = self
            .endpoint
            .connect(addr, b"theta")
            .await
            .map_err(|e| NetworkError::ConnectError(Arc::new(e)))?;

        Ok(Transport { conn })
    }

    pub(crate) async fn accept(&self) -> Result<(PublicKey, Transport), NetworkError> {
        let Some(incoming) = self.endpoint.accept().await else {
            return Err(NetworkError::PeerClosedWhileAccepting);
        };

        let conn = match incoming.await {
            Ok(conn) => conn,
            Err(e) => return Err(NetworkError::ConnectionError(Arc::new(e))),
        };

        let public_key = conn
            .remote_node_id()
            .expect("remote node ID should be present");

        Ok((public_key, Transport { conn }))
    }

    pub(crate) fn connect_and_prepare(&self, addr: NodeAddr) -> PreparedConn {
        let this = self.clone();

        let fut = async move {
            let transport = this.connect(addr).await?;

            // Open control streams
            let control_tx = transport.open_uni().await?;
            let control_rx = transport.accept_uni().await?;

            Ok(PreparedConnInner {
                transport,
                control_tx: Arc::new(Mutex::new(control_tx)),
                control_rx: Arc::new(Mutex::new(control_rx)),
            })
        }
        .boxed()
        .shared();

        PreparedConn { inner: fut }
    }

    pub(crate) async fn accept_and_prepare(
        &self,
    ) -> Result<(PublicKey, PreparedConn), NetworkError> {
        let (public_key, transport) = self.accept().await?;

        // Open control streams
        let control_rx = transport.accept_uni().await?;
        let control_tx = transport.open_uni().await?;

        let inner = async move {
            Ok(PreparedConnInner {
                transport,
                control_tx: Arc::new(Mutex::new(control_tx)),
                control_rx: Arc::new(Mutex::new(control_rx)),
            })
        }
        .boxed()
        .shared();

        Ok((public_key, PreparedConn { inner }))
    }
}

impl Transport {
    pub(crate) async fn open_uni(&self) -> Result<TxStream, NetworkError> {
        let tx_stream = self
            .conn
            .open_uni()
            .await
            .map_err(|e| NetworkError::ConnectionError(Arc::new(e)))?;

        Ok(TxStream(tx_stream))
    }

    pub(crate) async fn accept_uni(&self) -> Result<RxStream, NetworkError> {
        let rx_stream = self
            .conn
            .accept_uni()
            .await
            .map_err(|e| NetworkError::ConnectionError(Arc::new(e)))?;

        Ok(RxStream(rx_stream))
    }
}

impl TxStream {
    pub(crate) async fn send_frame(&mut self, data: Vec<u8>) -> Result<(), NetworkError> {
        self.0
            .write_u32(data.len() as u32)
            .await
            .map_err(|e| NetworkError::IoError(Arc::new(e)))?;

        self.0
            .write_all(&data)
            .await
            .map_err(|e| NetworkError::WriteError(Arc::new(e)))?;

        Ok(())
    }
}

impl RxStream {
    pub(crate) async fn recv_frame(&mut self) -> Result<Vec<u8>, NetworkError> {
        let len = self
            .0
            .read_u32()
            .await
            .map_err(|e| NetworkError::IoError(Arc::new(e)))?;

        let mut data = vec![0; len as usize];
        self.0
            .read_exact(&mut data)
            .await
            .map_err(|e| NetworkError::ReadExactError(Arc::new(e)))?;

        Ok(data)
    }
}

// This is what will be actually used

#[derive(Debug, Clone)]
pub(crate) struct PreparedConn {
    inner: Shared<BoxFuture<'static, Result<PreparedConnInner, NetworkError>>>,
}

#[derive(Debug, Clone)]
struct PreparedConnInner {
    transport: Transport,
    control_tx: Arc<Mutex<TxStream>>,
    control_rx: Arc<Mutex<RxStream>>,
}

impl PreparedConn {
    pub(crate) async fn send_datagram(&self, data: Vec<u8>) -> Result<(), NetworkError> {
        let inner = self.get().await?;

        let mut control_tx = inner.control_tx.lock().await;
        control_tx.send_frame(data).await
    }

    pub(crate) async fn recv_datagram(&self) -> Result<Vec<u8>, NetworkError> {
        let inner = self.get().await?;

        let mut control_rx = inner.control_rx.lock().await;
        control_rx.recv_frame().await
    }

    pub(crate) async fn open_uni(&self) -> Result<TxStream, NetworkError> {
        let inner = self.get().await?;
        inner.transport.open_uni().await
    }

    pub(crate) async fn accept_uni(&self) -> Result<RxStream, NetworkError> {
        let inner = self.get().await?;
        inner.transport.accept_uni().await
    }

    async fn get(&self) -> Result<PreparedConnInner, NetworkError> {
        self.inner.clone().await
    }
}
