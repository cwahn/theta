use std::{fmt::Debug, hash::Hash, marker::PhantomData, sync::Arc, time::Duration};

use futures::future::BoxFuture;
use tokio::sync::{
    Notify,
    mpsc::{self, UnboundedSender, WeakUnboundedSender},
    oneshot,
};
use uuid::Uuid;

use crate::{
    actor::Actor,
    error::{RequestError, SendError},
    message::{Behavior, Continuation, DynMessage, Escalation, InternalSignal, Message, RawSignal},
};

/// Address to an actor, capable of sending messages

#[derive(Debug)]
pub struct ActorRef<A: Actor>(
    pub(crate) Uuid,
    pub(crate) UnboundedSender<(DynMessage<A>, Option<Continuation>)>,
);

#[derive(Debug)]
pub struct WeakActorRef<A: Actor>(
    pub(crate) Uuid,
    pub(crate) WeakUnboundedSender<(DynMessage<A>, Option<Continuation>)>,
);

/// Type agnostic handle of an actor, capable of sending signal
#[derive(Debug, Clone)]
pub struct ActorHdl(pub(crate) UnboundedSender<RawSignal>);

/// Type agnostic handle for supervision, weak form
#[derive(Debug, Clone)]
pub struct WeakActorHdl(pub(crate) WeakUnboundedSender<RawSignal>);

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
    pub fn id(&self) -> Uuid {
        self.0
    }

    pub fn send<M>(
        &self,
        msg: M,
        k: Option<Continuation>,
    ) -> Result<(), SendError<(Box<dyn Message<A>>, Option<Continuation>)>>
    where
        M: Debug + Send + 'static,
        A: Behavior<M>,
    {
        self.1
            .send((Box::new(msg), k))
            .map_err(|e| SendError::ClosedTx(e.0))
    }

    pub fn tell<M>(&self, msg: M) -> Result<(), SendError<Box<dyn Message<A>>>>
    where
        M: Debug + Send + 'static,
        A: Behavior<M>,
    {
        self.1
            .send((Box::new(msg), None))
            .map_err(|e| SendError::ClosedTx(e.0.0))
    }

    pub fn ask<M>(&self, msg: M) -> MsgRequest<'_, A, M>
    where
        M: Send + 'static,
        A: Behavior<M>,
    {
        MsgRequest { target: self, msg }
    }

    pub fn downgrade(&self) -> WeakActorRef<A> {
        WeakActorRef(self.0, self.1.downgrade())
    }

    pub fn is_closed(&self) -> bool {
        self.1.is_closed()
    }
}

impl<A> Clone for ActorRef<A>
where
    A: Actor,
{
    fn clone(&self) -> Self {
        ActorRef(self.0, self.1.clone())
    }
}

impl<A> PartialEq for ActorRef<A>
where
    A: Actor,
{
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<A> Eq for ActorRef<A> where A: Actor {}

impl<A> Hash for ActorRef<A>
where
    A: Actor,
{
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl<A> PartialOrd for ActorRef<A>
where
    A: Actor,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.0.cmp(&other.0))
    }
}

impl<A> WeakActorRef<A>
where
    A: Actor,
{
    pub fn id(&self) -> Uuid {
        self.0
    }

    pub fn upgrade(&self) -> Option<ActorRef<A>> {
        self.1.upgrade().map(|tx| ActorRef(self.0, tx))
    }
}

impl<A> Clone for WeakActorRef<A>
where
    A: Actor,
{
    fn clone(&self) -> Self {
        WeakActorRef(self.0, self.1.clone())
    }
}

impl<A> PartialEq for WeakActorRef<A>
where
    A: Actor,
{
    fn eq(&self, other: &Self) -> bool {
        // ? Can this cause glitch suggesting wrong string count to the actor?
        self.0 == other.0
    }
}

impl<A> Eq for WeakActorRef<A> where A: Actor {}

impl<A> Hash for WeakActorRef<A>
where
    A: Actor,
{
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl<A> PartialOrd for WeakActorRef<A>
where
    A: Actor,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.0.cmp(&other.0))
    }
}

impl<A> Ord for WeakActorRef<A>
where
    A: Actor,
{
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
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
    M: Debug + Send + 'static,
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
    M: Debug + Send + 'static,
{
    type Output = Result<A::Return, RequestError<Box<dyn Message<A>>>>;
    type IntoFuture = BoxFuture<'a, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();

            let send_res = self.target.1.send((Box::new(self.msg), Some(tx)));

            if let Err(mpsc::error::SendError((msg, _))) = send_res {
                // Not to downcast, since most of the time it just dropped
                return Err(RequestError::ClosedTx(msg));
            }

            match rx.await {
                Ok(res) => {
                    let Ok(res) = res.downcast::<A::Return>() else {
                        return Err(RequestError::ClosedRx);
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
