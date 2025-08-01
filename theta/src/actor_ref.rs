use std::{fmt::Debug, hash::Hash, marker::PhantomData, sync::Arc, time::Duration};

use futures::{channel::oneshot, future::BoxFuture};
#[cfg(feature = "tracing")]
use tokio::sync::{
    Notify,
    mpsc::{self, UnboundedReceiver, UnboundedSender, WeakUnboundedSender},
};
use tracing::error;

use crate::{
    actor::{Actor, ActorId},
    error::{RequestError, SendError},
    message::{Behavior, Continuation, DynMessage, Escalation, InternalSignal, Message, RawSignal},
};

pub type MsgPack<A> = (DynMessage<A>, Continuation);

pub type MsgTx<A> = UnboundedSender<MsgPack<A>>;
pub type WeakMsgTx<A> = WeakUnboundedSender<MsgPack<A>>;
pub type MsgRx<A> = UnboundedReceiver<MsgPack<A>>;

pub type SigTx = UnboundedSender<RawSignal>;
pub type WeakSigTx = WeakUnboundedSender<RawSignal>;
pub type SigRx = UnboundedReceiver<RawSignal>;

/// Address to an actor, capable of sending messages
#[derive(Debug)]
pub struct ActorRef<A: Actor> {
    pub(crate) id: ActorId,
    pub(crate) tx: MsgTx<A>,
}

#[derive(Debug)]
pub struct WeakActorRef<A: Actor> {
    pub(crate) id: ActorId,
    pub(crate) tx: WeakMsgTx<A>,
}

/// Type agnostic handle of an actor, capable of sending signal
#[derive(Debug, Clone)]
pub struct ActorHdl(pub(crate) SigTx);

/// Type agnostic handle for supervision, weak form
#[derive(Debug, Clone)]
pub struct WeakActorHdl(pub(crate) WeakSigTx);

pub struct MsgRequest<'a, A, M>
where
    A: Actor + Behavior<M>,
    M: Send + 'static,
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

impl<A> ActorRef<A>
where
    A: Actor,
{
    pub fn id(&self) -> ActorId {
        self.id
    }

    pub fn is_nil(&self) -> bool {
        self.id.is_nil()
    }

    pub fn tell<M>(&self, msg: M) -> Result<(), SendError<(DynMessage<A>, Continuation)>>
    where
        M: Debug + Send + Sync + erased_serde::Serialize + 'static,
        A: Behavior<M>,
    {
        self.send_dyn(Box::new(msg), Continuation::nil())
    }

    pub fn ask<M>(&self, msg: M) -> MsgRequest<'_, A, M>
    where
        M: Send + 'static,
        A: Behavior<M>,
    {
        MsgRequest { target: self, msg }
    }

    // todo pub fn send<M, B>(
    //     &self,
    //     msg: M,
    //     forward: ActorRef<B>,
    // ) -> Result<(), SendError<(DynMessage<A>, Continuation)>>
    // where
    //     M: Debug + Send + erased_serde::Serialize + 'static,
    //     A: Behavior<M>,
    //     B: Actor + Behavior<<A as Behavior<M>>::Return>,
    //     <A as Behavior<M>>::Return: serde::Serialize,
    // {
    //     let (tx, rx) = oneshot::channel::<Box<dyn Any + Send>>();

    //     tokio::spawn(async move {
    //         let Ok(res) = rx.await else {
    //             return; // Cancelled
    //         };

    //         let Ok(b_msg) = res.downcast::<<A as Behavior<M>>::Return>() else {
    //             #[cfg(feature = "tracing")]
    //             error!(
    //                 "Failed to downcast response from actor {}: expected {}",
    //                 forward.id,
    //                 std::any::type_name::<<A as Behavior<M>>::Return>()
    //             );

    //             return; // Wrong type
    //         };

    //         let msg = DynMessage::from(b_msg);

    //         let _ = forward.send_dyn(msg, Continuation::nil());
    //     });

    //     let continuation = Continuation::forward(tx);

    //     self.send_dyn(Box::new(msg), continuation)
    // }

    pub fn downgrade(&self) -> WeakActorRef<A> {
        WeakActorRef {
            id: self.id,
            tx: self.tx.downgrade(),
        }
    }
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    pub(crate) fn send_dyn(
        &self,
        msg: DynMessage<A>,
        k: Continuation,
    ) -> Result<(), SendError<(DynMessage<A>, Continuation)>> {
        self.tx.send((msg, k)).map_err(|e| SendError::ClosedTx(e.0))
    }
}

impl<A> Clone for ActorRef<A>
where
    A: Actor,
{
    fn clone(&self) -> Self {
        ActorRef {
            id: self.id,
            tx: self.tx.clone(),
        }
    }
}

impl<A> PartialEq for ActorRef<A>
where
    A: Actor,
{
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<A> Eq for ActorRef<A> where A: Actor {}

impl<A> Hash for ActorRef<A>
where
    A: Actor,
{
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl<A> PartialOrd for ActorRef<A>
where
    A: Actor,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.id.cmp(&other.id))
    }
}

impl<A> WeakActorRef<A>
where
    A: Actor,
{
    pub fn id(&self) -> ActorId {
        self.id
    }

    pub fn upgrade(&self) -> Option<ActorRef<A>> {
        self.tx.upgrade().map(|tx| ActorRef { id: self.id, tx })
    }
}

impl<A> Clone for WeakActorRef<A>
where
    A: Actor,
{
    fn clone(&self) -> Self {
        WeakActorRef {
            id: self.id,
            tx: self.tx.clone(),
        }
    }
}

impl<A> PartialEq for WeakActorRef<A>
where
    A: Actor,
{
    fn eq(&self, other: &Self) -> bool {
        // ? Can this cause glitch suggesting wrong string count to the actor?
        self.id == other.id
    }
}

impl<A> Eq for WeakActorRef<A> where A: Actor {}

impl<A> Hash for WeakActorRef<A>
where
    A: Actor,
{
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl<A> PartialOrd for WeakActorRef<A>
where
    A: Actor,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.id.cmp(&other.id))
    }
}

impl<A> Ord for WeakActorRef<A>
where
    A: Actor,
{
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.id.cmp(&other.id)
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
    A: Actor + Behavior<M>,
    M: Debug + Send + Sync + erased_serde::Serialize + 'static,
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
    A: Actor + Behavior<M>,
    M: Debug + Send + Sync + erased_serde::Serialize + 'static,
{
    type Output = Result<A::Return, RequestError<Box<dyn Message<A>>>>;
    type IntoFuture = BoxFuture<'a, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();

            let send_res = self
                .target
                .tx
                .send((Box::new(self.msg), Continuation::reply(tx)));

            if let Err(mpsc::error::SendError((msg, _))) = send_res {
                // Not to downcast, since most of the time it just dropped
                return Err(RequestError::ClosedTx(msg));
            }

            match rx.await {
                Ok(res) => {
                    let res = match res.downcast::<A::Return>() {
                        Ok(res) => res, // Local reply
                        Err(res) => {
                            let Ok(remote_reply_rx) = res.downcast::<oneshot::Receiver<Vec<u8>>>()
                            else {
                                #[cfg(feature = "tracing")]
                                error!(
                                    "Initial reply should be either A::Return or oneshot::Receiver<Vec<u8>>"
                                );
                                return Err(RequestError::DowncastFail);
                            };

                            let Ok(bytes) = remote_reply_rx.await else {
                                return Err(RequestError::ClosedRx);
                            };

                            let Ok(res) = postcard::from_bytes::<A::Return>(&bytes) else {
                                #[cfg(feature = "tracing")]
                                error!("Failed to deserialize remote reply for actor");

                                return Err(RequestError::DeserializeFail);
                            };

                            Box::new(res)
                        }
                    };

                    Ok(*res)
                }
                Err(_) => Err(RequestError::ClosedRx),
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
