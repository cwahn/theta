use std::{
    fmt::Debug,
    pin::Pin,
    sync::OnceLock,
    task::{Context, Poll},
};

use futures::channel::oneshot::Canceled;
use theta_protocol::core::{Network, Receiver, Sender, Transport};
use thiserror::Error;
use tokio::{task::LocalKey, time::error::Elapsed};
use url::Url;
use uuid::Uuid;

use crate::{
    base::{Ident, Nil},
    context::{LookupError, ObserveError},
    remote::peer::{LocalPeer, Peer},
};

pub type ActorTypeId = Uuid;

pub trait Remote: 'static + Sized + Debug {
    type Network: 'static + Network;

    fn local_peer() -> &'static OnceLock<LocalPeer<Self>>;
    fn peer() -> &'static LocalKey<Peer<Self>>;
}

pub(crate) type ReplyKey = u64;
pub(crate) type Tag = u32;

pub(crate) type Net<R> = <R as Remote>::Network;
pub(crate) type Trans<R> = <<R as Remote>::Network as Network>::Transport;

pub(crate) type TxStream<R> = <<<R as Remote>::Network as Network>::Transport as Transport>::Sender;
pub(crate) type RxStream<R> =
    <<<R as Remote>::Network as Network>::Transport as Transport>::Receiver;

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

pub(crate) fn split_url(mut addr: Url) -> Result<(Url, Ident), RemoteError> {
    // The last sengment of path is actor identifer
    let ident = addr
        .path_segments()
        .and_then(|it| it.filter(|s| !s.is_empty()).next_back())
        .ok_or(RemoteError::InvalidAddress)?
        .as_bytes()
        .to_vec()
        .into();

    {
        let mut segs = addr
            .path_segments_mut()
            .map_err(|_| RemoteError::InvalidAddress)?;

        segs.pop_if_empty();
        segs.pop(); // remove last real segment
    }

    Ok((addr, ident))
}

// Implementation

impl Remote for Nil {
    type Network = Nil;

    fn local_peer() -> &'static OnceLock<LocalPeer<Self>> {
        unimplemented!()
    }

    fn peer() -> &'static LocalKey<Peer<Self>> {
        unimplemented!()
    }
}

impl Network for Nil {
    type Transport = Nil;

    fn connect(&self, _host_addr: &Url) -> Result<Self::Transport, theta_protocol::error::Error> {
        unimplemented!()
    }

    fn run(&self, _on_accept: fn(Self::Transport)) {
        unimplemented!()
    }

    fn is_supported_scheme(&self, _addr: &Url) -> bool {
        unimplemented!()
    }
}

impl Transport for Nil {
    type Sender = Nil;
    type Receiver = Nil;

    fn host_addr(
        &self,
    ) -> futures::future::BoxFuture<'_, Result<Url, theta_protocol::error::Error>> {
        unimplemented!()
    }

    fn open_uni(
        &self,
    ) -> futures::future::BoxFuture<'_, Result<Self::Sender, theta_protocol::error::Error>> {
        unimplemented!()
    }

    fn accept_uni(
        &self,
    ) -> futures::future::BoxFuture<'_, Result<Self::Receiver, theta_protocol::error::Error>> {
        unimplemented!()
    }

    fn recv_datagram(
        &self,
    ) -> futures::future::BoxFuture<'_, Result<Vec<u8>, theta_protocol::error::Error>> {
        unimplemented!()
    }

    fn send_datagram(
        &self,
        _payload: Vec<u8>,
    ) -> futures::future::BoxFuture<'_, Result<(), theta_protocol::error::Error>> {
        unimplemented!()
    }
}

impl Sender for Nil {
    type SendFrame<'a>
        = _NilSendFrame
    where
        Self: 'a;

    fn send_frame(&mut self, payload: Vec<u8>) -> Self::SendFrame<'_> {
        unimplemented!()
    }
}

impl Receiver for Nil {
    type RecvFrame<'a>
        = _NilRecvFrame
    where
        Self: 'a;

    fn recv_frame(&mut self) -> Self::RecvFrame<'_> {
        unimplemented!()
    }
}

pub struct _NilSendFrame;

pub struct _NilRecvFrame;

impl Future for _NilSendFrame {
    type Output = Result<(), theta_protocol::error::Error>;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        unimplemented!()
    }
}

impl Future for _NilRecvFrame {
    type Output = Result<Vec<u8>, theta_protocol::error::Error>;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        unimplemented!()
    }
}

// /// Logical sender
// /// It could be actual stream in case of WebSocket like transport, or internally wrap message with stream_id and send to single internal stream.
// pub trait Sender: Send + Sync {
//     type SendFrame<'a>: Future<Output = Result<(), Error>> + 'a
//     where
//         Self: 'a;

//     /// - The transport guarantees integrity‐checked, at‐most‐once delivery.
//     /// - The transport does not guarantee delivery or ordering
//     fn send_frame(&mut self, payload: Vec<u8>) -> Self::SendFrame<'_>;
// }

// /// Logical receiver
// /// It could be actual stream in case of WebSocket like transport, or internally wrap message from single internal stream.
// pub trait Receiver: Send + Sync {
//     type RecvFrame<'a>: Future<Output = Result<Vec<u8>, Error>> + 'a
//     where
//         Self: 'a;

//     /// Receive the next datagram from the peer.
//     fn recv_frame(&mut self) -> Self::RecvFrame<'_>;
// }
