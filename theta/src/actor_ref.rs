use std::{
    any::{Any, type_name},
    fmt::Debug,
    hash::Hash,
    marker::PhantomData,
    sync::Arc,
    time::Duration,
};

use futures::{
    channel::oneshot::{self, Canceled},
    future::BoxFuture,
};
use theta_flume::SendError;
use thiserror::Error;
use tokio::sync::Notify;

#[cfg(feature = "remote")]
use crate::remote::network::{IrohReceiver, IrohSender};
use crate::{
    actor::{Actor, ActorId},
    base::Ident,
    context::ObserveError,
    debug, error,
    message::{
        Continuation, Escalation, InternalSignal, Message, MsgPack, MsgTx, RawSignal, SigTx,
        WeakMsgTx, WeakSigTx,
    },
    monitor::AnyReportTx,
    remote::{
        base::{ActorTypeId, Tag},
        serde::FromTaggedBytes,
    },
};

#[cfg(feature = "remote")]
use {
    crate::{
        context::RootContext,
        monitor::Report,
        remote::{peer::{Peer, PEER}, serde::{ForwardInfo, MsgPackDto}},
        warn,
    },
    theta_flume::unbounded_anonymous,
};

pub(crate) trait AnyActorRef: Debug + Send + Sync + Any {
    fn as_any(&self) -> &dyn Any;

    fn id(&self) -> ActorId;

    fn send_tagged_bytes(&self, tag: Tag, bytes: Vec<u8>) -> Result<(), BytesSendError>;

    #[cfg(feature = "remote")]
    fn export_task_fn(
        &self,
    ) -> fn(Peer, IrohReceiver, Arc<dyn AnyActorRef>) -> BoxFuture<'static, ()>;

    #[cfg(feature = "remote")]
    fn observe_as_bytes(
        &self,
        peer: Peer,
        hdl: ActorHdl,
        bytes_tx: IrohSender,
    ) -> Result<(), ObserveError>;

    #[cfg(feature = "remote")]
    fn ty_id(&self) -> ActorTypeId;
}

/// Address of an actor or multiple actors, capable of sending messages
/// In case of actor pool, an actor reference connected to multiple actors
#[derive(Debug)]
pub struct ActorRef<A: Actor>(pub(crate) MsgTx<A>);

#[derive(Debug)]
pub struct WeakActorRef<A: Actor>(pub(crate) WeakMsgTx<A>);

/// Type agnostic handle of an actor, capable of sending signal
#[derive(Debug, Clone)]
pub struct ActorHdl(pub(crate) SigTx);

/// Type agnostic handle for supervision, weak form
#[derive(Debug, Clone)]
pub struct WeakActorHdl(pub(crate) WeakSigTx);

pub struct MsgRequest<'a, A, M>
where
    A: Actor,
    M: Send + Message<A> + 'static,
{
    target: &'a ActorRef<A>,
    msg: M,
}

// No escalationRequest, as it will comes back as a signal

pub struct SignalRequest<'a> {
    target_hdl: &'a ActorHdl,
    sig: InternalSignal,
}

pub struct Deadline<'a, R>
where
    R: IntoFuture + Send,
{
    request: R,
    duration: Duration,
    _phantom: PhantomData<&'a ()>,
}

#[derive(Debug, Error)]
pub enum BytesSendError {
    #[error(transparent)]
    DeserializeError(#[from] postcard::Error),
    #[error("send error: {0:#?}")]
    SendError((Tag, Vec<u8>)),
}

#[derive(Debug, Error)]
pub enum RequestError<T> {
    #[error(transparent)]
    Cancelled(#[from] Canceled),
    #[error(transparent)]
    SendError(#[from] SendError<T>),
    #[error("receiving on a closed channel")]
    RecvError,
    #[error("downcast failed")]
    DowncastError,
    #[error(transparent)]
    DeserializeError(#[from] postcard::Error),
    #[error("timeout")]
    Timeout,
}

// Implementations

impl<A: Actor + Any> AnyActorRef for ActorRef<A> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn id(&self) -> ActorId {
        self.0.id()
    }

    fn send_tagged_bytes(&self, tag: Tag, bytes: Vec<u8>) -> Result<(), BytesSendError> {
        let msg = <A::Msg as FromTaggedBytes>::from(tag, &bytes)?;

        self.send_raw(msg, Continuation::Nil)
            .map_err(|_| BytesSendError::SendError((tag, bytes)))
    }

    #[cfg(feature = "remote")]
    fn export_task_fn(
        &self,
    ) -> fn(Peer, IrohReceiver, Arc<dyn AnyActorRef>) -> BoxFuture<'static, ()> {
        |peer: Peer,
         mut in_stream: IrohReceiver,
         actor: Arc<dyn AnyActorRef>|
         -> BoxFuture<'static, ()> {
            Box::pin(PEER.scope(peer, async move {
                let Some(actor) = actor.as_any().downcast_ref::<ActorRef<A>>() else {
                    return error!(
                        "Failed to downcast any actor reference to {}",
                        type_name::<A>()
                    );
                };

                loop {
                    let bytes = match in_stream.recv_frame().await {
                        Ok(bytes) => bytes,
                        Err(e) => break error!("Failed to receive frame from stream: {e}")
                    };

                    let (msg, k_dto) = match postcard::from_bytes::<MsgPackDto<A>>(&bytes) {
                        Ok((msg, k_dto)) => (msg, k_dto),
                        Err(e) => {
                            warn!("Failed to deserialize msg pack dto: {e}");
                            continue;
                        }
                    };

                    let (msg, k): MsgPack<A> = (msg, k_dto.into());

                    if let Err(e) = actor.send_raw(msg, k) {
                        break error!("Failed to send message to actor: {e}");
                    }
                }
            }))
        }
    }

    #[cfg(feature = "remote")]
    fn observe_as_bytes(
        &self,
        peer: Peer,
        hdl: ActorHdl,
        mut bytes_tx: IrohSender,
    ) -> Result<(), ObserveError> {
        let (tx, rx) = unbounded_anonymous::<Report<A>>();

        tokio::spawn(PEER.scope(peer, async move {
            loop {
                let Some(report) = rx.recv().await else {
                    return warn!("Report channel is closed");
                };

                let bytes = match postcard::to_stdvec(&report) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        warn!("Failed to serialize report: {e}");
                        continue;
                    }
                };

                if let Err(e) = bytes_tx.send_frame(bytes).await {
                    break warn!("Failed to send report frame: {e}");
                }
            }
        }));

        hdl.observe(Box::new(tx))
            .map_err(|_| ObserveError::SigSendError)?;

        Ok(())
    }

    #[cfg(feature = "remote")]
    fn ty_id(&self) -> ActorTypeId {
        A::IMPL_ID
    }
}

impl<A> ActorRef<A>
where
    A: Actor,
{
    pub fn id(&self) -> ActorId {
        self.0.id()
    }

    pub fn ident(&self) -> Ident {
        self.0.id().to_bytes_le().to_vec().into()
    }

    pub fn is_nil(&self) -> bool {
        self.0.id().is_nil()
    }

    pub fn tell<M>(&self, msg: M) -> Result<(), SendError<(A::Msg, Continuation)>>
    where
        M: Message<A>,
    {
        self.send_raw(msg.into(), Continuation::Nil)
    }

    pub fn ask<M>(&self, msg: M) -> MsgRequest<'_, A, M>
    where
        M: Message<A>,
    {
        MsgRequest { target: self, msg }
    }

    pub fn forward<M, B>(
        &self,
        msg: M,
        target: ActorRef<B>,
    ) -> Result<(), SendError<(A::Msg, Continuation)>>
    where
        M: Message<A>,
        B: Actor,
        <M as Message<A>>::Return: Message<B>,
    {
        let (tx, rx) = oneshot::channel::<Box<dyn Any + Send>>();

        tokio::spawn(async move {
            let Ok(ret) = rx.await else {
                return; // Cancelled
            };

            let b_msg = match ret.downcast::<<M as Message<A>>::Return>() {
                Ok(b_msg) => b_msg,
                Err(ret) => {
                    #[cfg(not(feature = "remote"))]
                    {
                        return error!(
                            "Failed to downcast response from actor {}: expected {}",
                            forward_to.id(),
                            type_name::<<M as Message<A>>::Return>()
                        );
                    }

                    #[cfg(feature = "remote")]
                    {
                        use crate::remote::peer::LocalPeer;

                        let Ok(tx) = ret.downcast::<oneshot::Sender<ForwardInfo>>() else {
                            return error!(
                                "Failed to downcast initial response from actor {}: expected {} or oneshot::Sender<ForwardInfo>",
                                target.id(),
                                type_name::<<M as Message<A>>::Return>()
                            );
                        };

                        let forward_info = match LocalPeer::inst().get_import::<B>(&target.ident())
                        {
                            None => {
                                if !RootContext::is_bound_impl::<B>(&target.ident()) {
                                    RootContext::bind_impl(target.ident().clone(), target.clone());
                                }

                                // Local is always second_party remote with respect to the recipient
                                ForwardInfo::Remote {
                                    public_key: None,
                                    ident: target.ident(),
                                    tag: <<M as Message<A>>::Return as Message<B>>::TAG,
                                }
                            }
                            Some(import) => ForwardInfo::Remote {
                                public_key: Some(import.peer.public_key()),
                                ident: target.ident(),
                                tag: <<M as Message<A>>::Return as Message<B>>::TAG,
                            },
                        };

                        if tx.send(forward_info).is_err() {
                            error!("Failed to send forward info");
                        }

                        return debug!("Deligate forwarding task to actor {}", target.id());
                    }
                }
            };

            let _ = target.send_raw((*b_msg).into(), Continuation::Nil);
        });

        let continuation = Continuation::forward(tx);

        self.send_raw(msg.into(), continuation)
    }

    pub fn downgrade(&self) -> WeakActorRef<A> {
        WeakActorRef(self.0.downgrade())
    }

    pub fn is_closed(&self) -> bool {
        self.0.is_closed()
    }

    pub(crate) fn send_raw(
        &self,
        msg: A::Msg,
        k: Continuation,
    ) -> Result<(), SendError<(A::Msg, Continuation)>> {
        self.0.send((msg, k))
    }
}

impl<A> Clone for ActorRef<A>
where
    A: Actor,
{
    fn clone(&self) -> Self {
        ActorRef(self.0.clone())
    }
}

impl<A> PartialEq for ActorRef<A>
where
    A: Actor,
{
    fn eq(&self, other: &Self) -> bool {
        self.0.id() == other.0.id()
    }
}

impl<A> Eq for ActorRef<A> where A: Actor {}

impl<A> Hash for ActorRef<A>
where
    A: Actor,
{
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.id().hash(state);
    }
}

impl<A> PartialOrd for ActorRef<A>
where
    A: Actor,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.0.id().cmp(&other.0.id()))
    }
}

impl<A> WeakActorRef<A>
where
    A: Actor,
{
    pub fn upgrade(&self) -> Option<ActorRef<A>> {
        self.0.upgrade().map(|tx| ActorRef(tx))
    }
}

impl<A> Clone for WeakActorRef<A>
where
    A: Actor,
{
    fn clone(&self) -> Self {
        WeakActorRef(self.0.clone())
    }
}

impl ActorHdl {
    pub(crate) fn id(&self) -> ActorId {
        self.0.id()
    }

    pub(crate) fn signal(&self, sig: InternalSignal) -> SignalRequest<'_> {
        SignalRequest {
            target_hdl: self,
            sig,
        }
    }

    pub(crate) fn downgrade(&self) -> WeakActorHdl {
        WeakActorHdl(self.0.downgrade())
    }

    pub(crate) fn escalate(
        &self,
        this_hdl: ActorHdl,
        escalation: Escalation,
    ) -> Result<(), SendError<RawSignal>> {
        self.raw_send(RawSignal::Escalation(this_hdl, escalation))
    }

    pub(crate) fn observe(&self, tx: AnyReportTx) -> Result<(), SendError<RawSignal>> {
        self.raw_send(RawSignal::Observe(tx))
    }

    pub(crate) fn raw_send(&self, raw_sig: RawSignal) -> Result<(), SendError<RawSignal>> {
        self.0.send(raw_sig)
    }
}

impl PartialEq for ActorHdl {
    fn eq(&self, other: &Self) -> bool {
        self.0.same_channel(&other.0)
    }
}

impl WeakActorHdl {
    pub(crate) fn upgrade(&self) -> Option<ActorHdl> {
        Some(ActorHdl(self.0.upgrade()?))
    }
}

impl<'a, A, M> MsgRequest<'a, A, M>
where
    A: Actor,
    M: Message<A>,
{
    pub fn timeout(self, duration: Duration) -> Deadline<'a, Self> {
        Deadline {
            request: self,
            duration,
            _phantom: PhantomData,
        }
    }
}

impl<'a, A, M> IntoFuture for MsgRequest<'a, A, M>
where
    A: Actor,
    M: Message<A>,
{
    type Output = Result<<M as Message<A>>::Return, RequestError<MsgPack<A>>>;
    type IntoFuture = BoxFuture<'a, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();

            self.target
                .0
                .send((self.msg.into(), Continuation::reply(tx)))?;

            let ret = rx.await?;

            match ret.downcast::<M::Return>() {
                Ok(res) => Ok(*res), // Local reply
                Err(ret) => {
                    #[cfg(not(feature = "remote"))]
                    {
                        return Err(RequestError::DowncastError);
                    }
                    #[cfg(feature = "remote")]
                    {
                        let Ok(remote_reply_rx) = ret.downcast::<oneshot::Receiver<Vec<u8>>>()
                        else {
                            return Err(RequestError::DowncastError);
                        };

                        let bytes = remote_reply_rx.await?;

                        let res = postcard::from_bytes::<M::Return>(&bytes)?;

                        Ok(res)
                    }
                }
            }
        })
    }
}

impl<'a> SignalRequest<'a> {
    pub fn timeout(self, duration: Duration) -> Deadline<'a, Self> {
        Deadline {
            request: self,
            duration,
            _phantom: PhantomData,
        }
    }
}

impl<'a> IntoFuture for SignalRequest<'a> {
    type Output = Result<(), RequestError<RawSignal>>;
    type IntoFuture = BoxFuture<'a, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let k = Arc::new(Notify::new());

            let raw_sig = match self.sig {
                InternalSignal::Pause => RawSignal::Pause(Some(k.clone())),
                InternalSignal::Resume => RawSignal::Resume(Some(k.clone())),
                InternalSignal::Restart => RawSignal::Restart(Some(k.clone())),
                InternalSignal::Terminate => RawSignal::Terminate(Some(k.clone())),
            };

            self.target_hdl.raw_send(raw_sig)?;

            k.notified().await;

            Ok(())
        })
    }
}

impl<'a, R, M> IntoFuture for Deadline<'a, R>
where
    R: 'a + IntoFuture<Output = Result<M, RequestError<M>>> + Send,
    R::IntoFuture: Send, // Add this bound
{
    type Output = R::Output;
    type IntoFuture = BoxFuture<'a, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let res = tokio::time::timeout(self.duration, self.request).await;
            match res {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(RequestError::Timeout),
            }
        })
    }
}
