use std::{any::Any, sync::Arc, time::Duration};

use futures::{FutureExt, future::BoxFuture};
use tokio::sync::{Notify, mpsc, oneshot};

use crate::{
    actor::Actor,
    continuation::Continuation,
    error::MsgError,
    message::{Behavior, DynMessage, Message},
    signal::{RawSignal, Signal},
};

/// Address to an actor, capable of sending messages
/// Can send termination signals, but not others
#[derive(Debug)]
pub struct ActorRef<A: Actor> {
    // By definition actor ref does not have id, but it self is just a address
    pub(crate) main_tx: MainRef<A>,
    // Signal_tx is always equal or more than message_tx
    pub(crate) signal_tx: SupervisionRef,
}

/// Address to an actor,  weak version
#[derive(Debug)]
pub struct WeakActorRef<A: Actor> {
    // By definition actor ref does not have id, but it self is just a address
    pub(crate) main_tx: WeakMainRef<A>,
    // Signal_tx is always equal or more than message_tx
    pub(crate) signal_tx: WeakSupervisionRef,
}

#[derive(Debug)]
pub struct MainRef<A: Actor>(
    pub(crate) mpsc::UnboundedSender<(DynMessage<A>, Option<Continuation>)>,
);

#[derive(Debug)]
pub struct WeakMainRef<A: Actor>(
    pub(crate) mpsc::WeakUnboundedSender<(DynMessage<A>, Option<Continuation>)>,
);

/// Used to set ActorRef as a continuation
pub struct DynMainRef(pub(crate) mpsc::UnboundedSender<Box<dyn Any + Send>>);

/// Type agnostic reference for supervision signaling
#[derive(Debug, Clone)]
pub struct SupervisionRef(pub(crate) mpsc::UnboundedSender<RawSignal>);

/// Type agnostic reference, weak version
#[derive(Debug, Clone)]
pub struct WeakSupervisionRef(pub(crate) mpsc::WeakUnboundedSender<RawSignal>);

struct SendMessage<'a, A, M>
where
    A: Actor + Behavior<M>,
    M: Send + 'static,
{
    main_ref: &'a MainRef<A>,
    message: M,
    continuation: Option<Continuation>,
}

struct AskMessage<'a, A, M>
where
    A: Actor + Behavior<M>,
    M: Send + 'static,
{
    send: SendMessage<'a, A, M>,
}

struct SignalRequest<'a> {
    supervision_ref: &'a SupervisionRef,
    signal: Signal,
}

// Implementations

impl<A> ActorRef<A>
where
    A: Actor,
{
    pub fn downgrade(&self) -> WeakActorRef<A> {
        WeakActorRef {
            main_tx: self.main_tx.downgrade(),
            signal_tx: self.signal_tx.downgrade(),
        }
    }
}

impl<A> Clone for ActorRef<A>
where
    A: Actor,
{
    fn clone(&self) -> Self {
        ActorRef {
            main_tx: self.main_tx.clone(),
            signal_tx: self.signal_tx.clone(),
        }
    }
}

impl<A> PartialEq for ActorRef<A>
where
    A: Actor,
{
    fn eq(&self, other: &Self) -> bool {
        self.signal_tx == other.signal_tx
    }
}

impl<A> WeakActorRef<A>
where
    A: Actor,
{
    pub fn upgrade(&self) -> Option<ActorRef<A>> {
        let message_tx = self.main_tx.upgrade()?;
        let signal_tx = self.signal_tx.upgrade()?;
        Some(ActorRef {
            main_tx: message_tx,
            signal_tx,
        })
    }
}

impl<A> Clone for WeakActorRef<A>
where
    A: Actor,
{
    fn clone(&self) -> Self {
        WeakActorRef {
            main_tx: self.main_tx.clone(),
            signal_tx: self.signal_tx.clone(),
        }
    }
}

impl<A> PartialEq for WeakActorRef<A>
where
    A: Actor,
{
    fn eq(&self, other: &Self) -> bool {
        match (&self.main_tx.0.upgrade(), &other.main_tx.0.upgrade()) {
            (Some(lhs), Some(rhs)) => lhs.same_channel(&rhs),
            _ => false,
        }
    }
}

impl<A> MainRef<A>
where
    A: Actor,
{
    pub fn tell(
        &self,
        message: DynMessage<A>,
    ) -> Result<(), mpsc::error::SendError<(DynMessage<A>, Option<Continuation>)>> {
        self.0.send((message, None))
    }

    pub fn ask<M>(&self, message: M) -> SendMessage<A>
    where
        M: Message<A> + Send + 'static,
    {
    }

    pub fn send(
        &self,
        message: DynMessage<A>,
        continuation: Option<Continuation>,
    ) -> SendMessage<'_, A> {
        SendMessage {
            main_ref: self,
            message,
            continuation,
        }
    }

    pub fn downgrade(&self) -> WeakMainRef<A> {
        WeakMainRef(self.0.downgrade())
    }
}

impl<A> Clone for MainRef<A>
where
    A: Actor,
{
    fn clone(&self) -> Self {
        MainRef(self.0.clone())
    }
}

impl<A> WeakMainRef<A>
where
    A: Actor,
{
    pub fn upgrade(&self) -> Option<MainRef<A>> {
        Some(MainRef(self.0.upgrade()?))
    }
}

impl<A> Clone for WeakMainRef<A>
where
    A: Actor,
{
    fn clone(&self) -> Self {
        WeakMainRef(self.0.clone())
    }
}

impl DynMainRef {
    pub fn send(self, value: Box<dyn Any + Send>) -> Result<(), MsgError<Box<dyn Any + Send>>> {
        match self.0.send(value) {
            Ok(_) => Ok(()),
            Err(x) => Err(MsgError::LocalClosed(x.0)),
        }
    }
}

impl SupervisionRef {
    pub fn tell(&self, signal: Signal) -> Result<(), mpsc::error::SendError<RawSignal>> {
        self.0.send(signal.into())
    }

    pub fn ask(&self, signal: Signal) -> SignalRequest {
        SignalRequest {
            supervision_ref: self,
            signal,
        }
    }

    pub fn downgrade(&self) -> WeakSupervisionRef {
        WeakSupervisionRef(self.0.downgrade())
    }

    pub(crate) fn send(
        &self,
        raw_signal: RawSignal,
    ) -> Result<(), mpsc::error::SendError<RawSignal>> {
        self.0.send(raw_signal)
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

impl<'a, A, M> IntoFuture for SendMessage<'a, A, M>
where
    A: Actor + Behavior<M>,
    M: Send + 'static,
{
    type Output = Result<(), mpsc::error::SendError<(DynMessage<A>, Option<Continuation>)>>;
    type IntoFuture = BoxFuture<'a, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        async move {
            self.main_ref
                .0
                .send((Box::new(self.message), self.continuation))
        }
        .boxed()
    }
}

impl<'a, A, M> IntoFuture for AskMessage<'a, A, M>
where
    A: Actor + Behavior<M>,
    M: Send + 'static,
{
    type Output = Result<(), mpsc::error::SendError<(DynMessage<A>, Option<Continuation>)>>;
    type IntoFuture = BoxFuture<'a, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        async move {
            let (tx, rx) = oneshot::channel();

            let send_res = self.send.await;
        }
        .boxed()
    }
}

impl<'a> IntoFuture for SignalRequest<'a> {
    type Output = Result<(), mpsc::error::SendError<RawSignal>>;
    type IntoFuture = BoxFuture<'a, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        async move {
            let k = Arc::new(Notify::new());

            let raw_signal = match self.signal {
                Signal::Resume => RawSignal::Resume(Some(k.clone())),
                Signal::Restart => RawSignal::Restart(Some(k.clone())),
                Signal::Terminate => RawSignal::Terminate(Some(k.clone())),
                Signal::Abort => RawSignal::Abort(Some(k.clone())),
            };

            self.supervision_ref.0.send(raw_signal)?;

            k.notified().await;

            Ok(())
        }
        .boxed()
    }
}
