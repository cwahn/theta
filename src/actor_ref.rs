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
    message::{Behavior, Continuation, DynMessage, Message},
    signal::{Escalation, RawSignal, Signal},
};

/// Address to an actor, capable of sending messages

#[derive(Debug)]
pub struct ActorRef<A: Actor>(pub(crate) UnboundedSender<(DynMessage<A>, Option<Continuation>)>);

#[derive(Debug)]
pub struct WeakActorRef<A: Actor>(
    pub(crate) WeakUnboundedSender<(DynMessage<A>, Option<Continuation>)>,
);

/// Type agnostic reference for supervision signaling
#[derive(Debug, Clone)]
pub struct SupervisionRef(pub(crate) UnboundedSender<RawSignal>);

/// Type agnostic reference, weak version
#[derive(Debug, Clone)]
pub struct WeakSupervisionRef(pub(crate) WeakUnboundedSender<RawSignal>);

struct MsgRequest<'a, A, M>
where
    A: Actor + Behavior<M>,
    M: Send + 'static,
{
    target_ref: &'a ActorRef<A>,
    message: M,
}

// No escalationRequest, as it will comes back as a signal

struct SignalRequest<'a> {
    supervision_ref: &'a SupervisionRef,
    signal: Signal,
}

struct Deadline<'a, R>
where
    R: IntoFuture + Send,
{
    request: R,
    deadline: Duration,
    _phantom: PhantomData<&'a ()>,
}

// Implementations

impl<A> ActorRef<A>
where
    A: Actor,
{
    pub fn send<M>(
        &self,
        message: M,
        k: Option<Continuation>,
    ) -> Result<(), SendError<(Box<dyn Message<A>>, Option<Continuation>)>>
    where
        M: Send + 'static,
        A: Behavior<M>,
    {
        self.0
            .send((Box::new(message), k))
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
        MsgRequest {
            target_ref: self,
            message: msg,
        }
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
        match (&self.0.upgrade(), &other.0.upgrade()) {
            (Some(lhs), Some(rhs)) => lhs.same_channel(&rhs),
            _ => false,
        }
    }
}

impl SupervisionRef {
    pub fn signal(&self, signal: Signal) -> SignalRequest<'_> {
        SignalRequest {
            supervision_ref: self,
            signal,
        }
    }

    pub fn downgrade(&self) -> WeakSupervisionRef {
        WeakSupervisionRef(self.0.downgrade())
    }

    pub(crate) fn escalate(&self, escalation: Escalation) -> Result<(), SendError<RawSignal>> {
        self.raw_send(RawSignal::Escalation(self.clone(), escalation))
    }

    pub(crate) fn raw_send(&self, raw_signal: RawSignal) -> Result<(), SendError<RawSignal>> {
        self.0
            .send(raw_signal)
            .map_err(|e| SendError::ClosedTx(e.0))
    }
}

impl PartialEq for SupervisionRef {
    fn eq(&self, other: &Self) -> bool {
        self.0.same_channel(&other.0)
    }
}

impl WeakSupervisionRef {
    pub fn upgrade(&self) -> Option<SupervisionRef> {
        Some(SupervisionRef(self.0.upgrade()?))
    }
}

impl PartialEq for WeakSupervisionRef {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0.upgrade(), &other.0.upgrade()) {
            (Some(lhs), Some(rhs)) => lhs.same_channel(&rhs),
            _ => false,
        }
    }
}

impl<'a, A, M> MsgRequest<'a, A, M>
where
    A: Actor + Behavior<M>,
    M: Send + 'static,
{
    pub fn timeout(self, timeout: Duration) -> Deadline<'a, Self> {
        Deadline {
            request: self,
            deadline: timeout,
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

            let send_res = self.target_ref.0.send((Box::new(self.message), Some(tx)));

            if let Err(mpsc::error::SendError((msg, _))) = send_res {
                // Not to downcast, since most of the time it just dropped
                return Err(RequestError::ClosedTx(msg));
            }

            match rx.await {
                Ok(result) => {
                    let Ok(result) = result.downcast::<A::Return>() else {
                        return Err(RequestError::ClosedRx);
                    };

                    Ok(*result)
                }
                Err(_) => Err(RequestError::ClosedRx),
            }
        })
    }
}

impl<'a> SignalRequest<'a> {
    pub fn timeout(self, timeout: Duration) -> Deadline<'a, Self> {
        Deadline {
            request: self,
            deadline: timeout,
            _phantom: PhantomData,
        }
    }
}

impl<'a> IntoFuture for SignalRequest<'a> {
    type Output = Result<(), RequestError<RawSignal>>;
    type IntoFuture = BoxFuture<'a, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let notify = Arc::new(Notify::new());

            let raw_signal = match self.signal {
                Signal::Pause => RawSignal::Pause(Some(notify.clone())),
                Signal::Resume => RawSignal::Resume(Some(notify.clone())),
                Signal::Restart => RawSignal::Restart(Some(notify.clone())),
                Signal::Terminate => RawSignal::Terminate(Some(notify.clone())),
            };

            self.supervision_ref.raw_send(raw_signal)?;

            notify.notified().await;

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
            let res = tokio::time::timeout(self.deadline, self.request).await;
            match res {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(RequestError::Timeout),
            }
        })
    }
}
