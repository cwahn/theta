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
    #[error(transparent)]
    ConnectionError(#[from] Arc<iroh::endpoint::ConnectionError>),
    #[error("peer closed while accepting")]
    PeerClosedWhileAccepting,
    #[error(transparent)]
    IoError(#[from] Arc<std::io::Error>),
    #[error(transparent)]
    ReadExactError(#[from] Arc<iroh::endpoint::ReadExactError>),
    #[error(transparent)]
    WriteError(#[from] Arc<iroh::endpoint::WriteError>),
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

    // todo Dedup
    pub(crate) async fn connect(&self, addr: NodeAddr) -> Result<Transport, NetworkError> {
        // Should reserve the connection first
        // If there is existing one, it means just accepted new incoming connection in the split secone
        // It should return existing connection
        // ? Is there any way I can do this without maintaing two map of connections?
        // It has to hold not only pending connections but established connections as well

        let conn = self
            .endpoint
            .connect(addr, b"theta")
            .await
            .map_err(|e| NetworkError::ConnectError(Arc::new(e)))?;

        // When the connection is revoked, it should check if there is any incoming connection
        // If so should wait for it to be accepted and return that instead
        // Otherwise, just return the error

        Ok(Transport { conn })
    }

    // todo Dedup
    pub(crate) async fn accept(&self) -> Result<(PublicKey, Transport), NetworkError> {
        let Some(incoming) = self.endpoint.accept().await else {
            return Err(NetworkError::PeerClosedWhileAccepting);
        };

        // todo Try to reserve the connection.
        // If there is existing one, there are two cases.
        // Reserved
        //  In case of reserved, it means there is out going attempt which is not finished.
        //  If one got incomming, that means counter part already reserved my public key.
        //  Counter part will do the dedup as well
        //  So it needs to predict the result, which could be done quite easily by using larger pk -> smaller pk
        //  If my pk is smaller, accept.
        //  If my pk is larger, should revoke and return none
        // Connected
        //  Is this case possible?

        let conn = match incoming.await {
            Err(e) => return Err(NetworkError::ConnectionError(Arc::new(e))),
            Ok(conn) => conn,
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

            let control_tx = transport.open_uni().await?;

            Ok(PreparedConnInner {
                transport,
                control_tx: Arc::new(Mutex::new(control_tx)),
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

        let control_tx = transport.open_uni().await?;

        let inner = async move {
            Ok(PreparedConnInner {
                transport,
                control_tx: Arc::new(Mutex::new(control_tx)),
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
    pub(crate) async fn send_frame(&mut self, data: &[u8]) -> Result<(), NetworkError> {
        // todo Add too long data error
        self.0
            .write_u32(data.len() as u32)
            .await
            .map_err(|e| NetworkError::IoError(Arc::new(e)))?;

        self.0
            .write_all(data)
            .await
            .map_err(|e| NetworkError::WriteError(Arc::new(e)))?;
        Ok(())
    }
}

impl RxStream {
    /// Receive a frame into a reusable buffer, allocating only if capacity is insufficient.
    /// - ! Expects cleared buffer
    pub(crate) async fn recv_frame_into(&mut self, buf: &mut Vec<u8>) -> Result<(), NetworkError> {
        let len = self
            .0
            .read_u32()
            .await
            .map_err(|e| NetworkError::IoError(Arc::new(e)))? as usize;

        if buf.capacity() < len {
            buf.reserve(len - buf.capacity());
        }

        // Safety: just allocated enough space
        unsafe { buf.set_len(len) }

        self.0
            .read_exact(buf)
            .await
            .map_err(|e| NetworkError::ReadExactError(Arc::new(e)))?;

        Ok(())
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
}

impl PreparedConn {
    pub(crate) async fn send_frame(&self, data: &[u8]) -> Result<(), NetworkError> {
        let inner = self.get().await?;

        inner.control_tx.lock().await.send_frame(data).await
    }

    // ! Should be called only once
    pub(crate) async fn control_rx(&self) -> Result<RxStream, NetworkError> {
        let inner = self.get().await?;

        inner.transport.accept_uni().await
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
