use std::sync::Arc;

use futures::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use iroh::{
    Endpoint, NodeAddr,
    endpoint::{Connection, RecvStream, SendStream},
};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Clone)]
pub struct IrohNetwork {
    endpoint: Endpoint,
}

#[derive(Debug, Clone)]
pub struct IrohTransport {
    inner: Shared<BoxFuture<'static, Result<Connection, NetworkError>>>,
}

#[derive(Debug)]
pub struct IrohSender(SendStream);

#[derive(Debug)]
pub struct IrohReceiver(RecvStream);

#[derive(Debug, Clone, Error)]
pub enum NetworkError {
    #[error(transparent)]
    ConnectError(#[from] Arc<iroh::endpoint::ConnectError>),
    #[error(transparent)]
    SendDatagramError(#[from] Arc<iroh::endpoint::SendDatagramError>),
    #[error(transparent)]
    ConnectionError(#[from] Arc<iroh::endpoint::ConnectionError>),
    #[error(transparent)]
    IoError(#[from] Arc<std::io::Error>),
    #[error(transparent)]
    WriteError(#[from] Arc<iroh::endpoint::WriteError>),
    #[error(transparent)]
    ReadExactError(#[from] Arc<iroh::endpoint::ReadExactError>),
}

// Implementation

impl IrohNetwork {
    pub(crate) fn new(endpoint: iroh::Endpoint) -> Self {
        Self { endpoint }
    }

    pub(crate) fn public_key(&self) -> iroh::PublicKey {
        self.endpoint.node_id()
    }

    pub(crate) fn connect(&self, addr: NodeAddr) -> IrohTransport {
        let endpoint = self.endpoint.clone();
        let inner = async move {
            endpoint
                .connect(addr, b"theta")
                .await
                .map_err(|e| NetworkError::ConnectError(Arc::new(e)))
        }
        .boxed()
        .shared();

        IrohTransport { inner }
    }
}

impl IrohTransport {
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

    pub(crate) async fn open_uni(&self) -> Result<IrohSender, NetworkError> {
        let conn = self.get().await?;

        let tx_stream = conn
            .open_uni()
            .await
            .map_err(|e| NetworkError::ConnectionError(Arc::new(e)))?;

        Ok(IrohSender(tx_stream))
    }

    pub(crate) async fn accept_uni(&self) -> Result<IrohReceiver, NetworkError> {
        let conn = self.get().await?;

        let rx_stream = conn
            .accept_uni()
            .await
            .map_err(|e| NetworkError::ConnectionError(Arc::new(e)))?;

        Ok(IrohReceiver(rx_stream))
    }

    async fn get(&self) -> Result<Connection, NetworkError> {
        Ok(self.inner.clone().await?)
    }
}

impl IrohSender {
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

impl IrohReceiver {
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
