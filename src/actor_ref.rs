use std::{marker::PhantomData, sync::Arc, time::Duration};

use futures::future::BoxFuture;
use tokio::sync::{
    Notify,
    mpsc::{self, UnboundedSender, WeakUnboundedSender},
    oneshot,
};

use crate::{
    actor::Actor,
    error::{RequestError, SendError},
    message::{Behavior, Continuation, DynMessage, Escalation, Message, RawSignal, Signal},
};

/// Address to an actor, capable of sending messages

#[derive(Debug)]
pub struct ActorRef<A: Actor>(pub(crate) UnboundedSender<(DynMessage<A>, Option<Continuation>)>);

#[derive(Debug)]
pub struct WeakActorRef<A: Actor>(
    pub(crate) WeakUnboundedSender<(DynMessage<A>, Option<Continuation>)>,
);

/// Type agnostic handle of an actor, capable of sending signal
#[derive(Debug)]
pub struct ActorHdl(pub(crate) UnboundedSender<RawSignal>);

/// Type agnostic handle for supervision, weak form
#[derive(Debug)]
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
    sig: Signal,
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
    pub fn send<M>(
        &self,
        msg: M,
        k: Option<Continuation>,
    ) -> Result<(), SendError<(Box<dyn Message<A>>, Option<Continuation>)>>
    where
        M: Send + 'static,
        A: Behavior<M>,
    {
        self.0
            .send((Box::new(msg), k))
            .map_err(|e| SendError::ClosedTx(e.0))
    }

    pub fn tell<M>(&self, msg: M) -> Result<(), SendError<Box<dyn Message<A>>>>
    where
        M: Send + 'static,
        A: Behavior<M>,
    {
        self.0
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
        WeakActorRef(self.0.downgrade())
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
        self.0.same_channel(&other.0)
    }
}

impl<A> WeakActorRef<A>
where
    A: Actor,
{
    pub fn upgrade(&self) -> Option<ActorRef<A>> {
        Some(ActorRef(self.0.upgrade()?))
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

impl<A> PartialEq for WeakActorRef<A>
where
    A: Actor,
{
    fn eq(&self, other: &Self) -> bool {
        // ? Can this cause glitch suggesting wrong string count to the actor?
        match (&self.0.upgrade(), &other.0.upgrade()) {
            (Some(lhs), Some(rhs)) => lhs.same_channel(&rhs),
            _ => false,
        }
    }
}

impl ActorHdl {
    pub fn signal(&self, sig: Signal) -> SignalRequest<'_> {
        SignalRequest {
            target_hdl: self,
            sig,
        }
    }

    pub(crate) fn downgrade(&self) -> WeakActorHdl {
        WeakActorHdl(self.0.downgrade())
    }

    // User can not clone ActorHdl
    pub(crate) fn clone(&self) -> Self {
        ActorHdl(self.0.clone())
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
    M: Send + 'static,
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
    M: Send + 'static,
{
    type Output = Result<A::Return, RequestError<Box<dyn Message<A>>>>;
    type IntoFuture = BoxFuture<'a, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();

            let send_res = self.target.0.send((Box::new(self.msg), Some(tx)));

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
                Signal::Pause => RawSignal::Pause(Some(k.clone())),
                Signal::Resume => RawSignal::Resume(Some(k.clone())),
                Signal::Restart => RawSignal::Restart(Some(k.clone())),
                Signal::Terminate => RawSignal::Terminate(Some(k.clone())),
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
