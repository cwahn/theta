use std::{
    any::{Any, type_name},
    fmt::Debug,
    hash::Hash,
    marker::PhantomData,
    sync::Arc,
    time::Duration,
};

use futures::{channel::oneshot, future::BoxFuture};
use theta_protocol::core::Ident;
use tokio::sync::Notify;

use crate::{
    actor::{Actor, ActorId},
    debug, error,
    errors::{RequestError, SendError},
    message::{
        Continuation, Escalation, InternalSignal, Message, MsgTx, RawSignal, SigTx, WeakMsgTx,
        WeakSigTx,
    },
    monitor::AnyReportTx,
    remote::{base::Tag, serde::FromTaggedBytes},
};

#[cfg(feature = "remote")]
use crate::{
    context::{BINDINGS, Binding},
    remote::{peer::RemoteActorExt, serde::ForwardInfo},
};

pub(crate) trait AnyActorRef: Debug + Send + Sync + Any {
    fn send_tagged_bytes(&self, tag: Tag, bytes: Vec<u8>) -> Result<(), SendError<(Tag, Vec<u8>)>>;

    fn as_any(&self) -> &dyn Any;
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

// Implementations

impl<A: Actor + Any> AnyActorRef for ActorRef<A> {
    fn send_tagged_bytes(&self, tag: Tag, bytes: Vec<u8>) -> Result<(), SendError<(Tag, Vec<u8>)>> {
        let msg = match <A::Msg as FromTaggedBytes>::from(tag, &bytes) {
            Ok(msg) => msg,
            Err(e) => {
                error!("Failed to deserialize message from tagged bytes: {e}");
                return Err(SendError::DeserializeFail(e));
            }
        };

        self.send_raw(msg, Continuation::Nil)
            .map_err(|_| SendError::ClosedTx((tag, bytes)))
    }

    fn as_any(&self) -> &dyn Any {
        self
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
        forward_to: ActorRef<B>,
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
                                forward_to.id(),
                                type_name::<<M as Message<A>>::Return>()
                            );
                        };

                        let forward_info = match LocalPeer::inst()
                            .get_import::<B>(&forward_to.ident())
                        {
                            None => {
                                if let None =
                                    BINDINGS.read().unwrap().get(forward_to.ident().as_ref())
                                {
                                    BINDINGS.write().unwrap().insert(
                                        forward_to.ident(),
                                        Binding {
                                            actor: Arc::new(forward_to.clone()),
                                            export_task_fn: <B as RemoteActorExt>::export_task_fn,
                                        },
                                    );
                                }

                                // Local is always second_party remote with respect to the recipient
                                ForwardInfo::Remote {
                                    host_addr: None,
                                    ident: forward_to.ident(),
                                    tag: <<M as Message<A>>::Return as Message<B>>::TAG,
                                }
                            }
                            Some(import) => ForwardInfo::Remote {
                                host_addr: Some(import.peer.host_addr()),
                                ident: forward_to.ident(),
                                tag: <<M as Message<A>>::Return as Message<B>>::TAG,
                            },
                        };

                        if let Err(_) = tx.send(forward_info) {
                            error!("Failed to send forward info");
                        }

                        return debug!("Deligate forwarding task to actor {}", forward_to.id());
                    }
                }
            };

            let _ = forward_to.send_raw((*b_msg).into(), Continuation::Nil);
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
        self.0.send((msg, k)).map_err(|e| SendError::ClosedTx(e.0))
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

    pub(crate) fn observer(&self, tx: AnyReportTx) -> Result<(), SendError<RawSignal>> {
        self.raw_send(RawSignal::Observe(tx))
    }

    pub(crate) fn raw_send(&self, raw_sig: RawSignal) -> Result<(), SendError<RawSignal>> {
        self.0.send(raw_sig).map_err(|e| SendError::ClosedTx(e.0))
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
    type Output = Result<<M as Message<A>>::Return, RequestError<<A as Actor>::Msg>>;
    type IntoFuture = BoxFuture<'a, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();

            let send_res = self
                .target
                .0
                .send((self.msg.into(), Continuation::reply(tx)));

            if let Err(theta_flume::SendError((msg, _))) = send_res {
                // Not to downcast, since most of the time it just dropped
                return Err(RequestError::ClosedTx(msg));
            }

            match rx.await {
                Err(_) => Err(RequestError::ClosedRx),
                Ok(ret) => {
                    match ret.downcast::<M::Return>() {
                        Ok(res) => Ok(*res), // Local reply
                        Err(ret) => {
                            #[cfg(not(feature = "remote"))]
                            {
                                errors!("Initial reply should be either A::Return");
                                return Err(RequestError::DowncastFail);
                            }
                            #[cfg(feature = "remote")]
                            {
                                let Ok(remote_reply_rx) =
                                    ret.downcast::<oneshot::Receiver<Vec<u8>>>()
                                else {
                                    error!(
                                        "Initial reply should be either A::Return or oneshot::Receiver<Vec<u8>>"
                                    );
                                    return Err(RequestError::DowncastFail);
                                };

                                let Ok(bytes) = remote_reply_rx.await else {
                                    return Err(RequestError::ClosedRx);
                                };

                                let res = match postcard::from_bytes::<M::Return>(&bytes) {
                                    Ok(res) => res,
                                    Err(e) => {
                                        error!("Failed to deserialize remote reply for actor: {e}");
                                        return Err(RequestError::DeserializeFail(e));
                                    }
                                };

                                Ok(res)
                            }
                        }
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
