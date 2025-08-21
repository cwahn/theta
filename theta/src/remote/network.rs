use std::sync::Arc;

use futures::{
    FutureExt,
    future::{BoxFuture, Shared},
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
    SendDatagramError(#[from] Arc<iroh::endpoint::SendDatagramError>),
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
    inner: Shared<BoxFuture<'static, Result<Connection, NetworkError>>>,
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

    pub(crate) fn connect(&self, addr: NodeAddr) -> Transport {
        let endpoint = self.endpoint.clone();
        let inner = async move {
            endpoint
                .connect(addr, b"theta")
                .await
                .map_err(|e| NetworkError::ConnectError(Arc::new(e)))
        }
        .boxed()
        .shared();

        Transport { inner }
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

        let inner = async move { Ok(conn) }.boxed().shared();

        Ok((public_key, Transport { inner }))
    }
}

impl Transport {
    pub(crate) async fn send_datagram(&self, data: Vec<u8>) -> Result<(), NetworkError> {
        let conn = self.get().await?;

        conn.send_datagram(data.into())
            .map_err(|e| NetworkError::SendDatagramError(Arc::new(e)))?;

        Ok(())
    }

    pub(crate) async fn recv_datagram(&self) -> Result<Vec<u8>, NetworkError> {
        let conn = self.get().await?;

        let data = conn
            .read_datagram()
            .await
            .map_err(|e| NetworkError::ConnectionError(Arc::new(e)))?;

        Ok(data.into())
    }

    pub(crate) async fn open_uni(&self) -> Result<TxStream, NetworkError> {
        let conn = self.get().await?;

        let tx_stream = conn
            .open_uni()
            .await
            .map_err(|e| NetworkError::ConnectionError(Arc::new(e)))?;

        Ok(TxStream(tx_stream))
    }

    pub(crate) async fn accept_uni(&self) -> Result<RxStream, NetworkError> {
        let conn = self.get().await?;

        let rx_stream = conn
            .accept_uni()
            .await
            .map_err(|e| NetworkError::ConnectionError(Arc::new(e)))?;

        Ok(RxStream(rx_stream))
    }

    async fn get(&self) -> Result<Connection, NetworkError> {
        self.inner.clone().await
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
