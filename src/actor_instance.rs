use std::{panic::AssertUnwindSafe, sync::Arc};

use futures::{FutureExt, lock::Mutex};
use tokio::sync::{Notify, RwLock, mpsc::UnboundedReceiver};

use crate::{
    actor::Actor,
    actor_ref::{SupervisionRef, WeakActorRef, WeakSupervisionRef},
    actor_status::{ActorStatus, MessageLoopStatus},
    base::{Immutable, panic_msg},
    context::{Context, SupervisionContext, WeakContext, WeakSupervisionContext},
    continuation::Continuation,
    message::DynMessage,
    signal::{Escalation, RawSignal},
};

pub(crate) struct ActorInstance<A>
where
    A: Actor,
{
    parent_ref: SupervisionRef,
    self_ref: WeakActorRef<A>,
    child_refs: Arc<Mutex<Vec<WeakSupervisionRef>>>, // children of this actor
    status: Arc<RwLock<ActorStatus<A>>>,
}

impl<A> ActorInstance<A>
where
    A: Actor,
{
    pub fn new(parent_ref: SupervisionRef, self_ref: WeakActorRef<A>) -> Self {
        Self {
            parent_ref,
            self_ref,
            child_refs: Arc::new(Mutex::new(Vec::new())),
            status: Arc::new(RwLock::new(ActorStatus::Starting)),
        }
    }

    pub(crate) async fn run(
        self,
        args: A::Args,
        message_rx: UnboundedReceiver<(DynMessage<A>, Option<Continuation>)>,
        signal_rx: UnboundedReceiver<RawSignal>,
    ) -> ActorStatus<A> {
        let actor = match self.start(args).await {
            Ok(actor) => actor,
            Err(err) => return ActorStatus::PanicOnStart(err),
        };

        self.set_status(ActorStatus::Running).await;
        let resume = Arc::new(Notify::new());

        let main_task = tokio::spawn(Self::main_loop(
            actor,
            self.get_weak_supervision_context(),
            self.status.clone(),
            resume.clone(),
            message_rx,
        ));

        let signal_task = tokio::spawn(Self::signal_loop(
            main_task.abort_handle(),
            self.get_weak_context(),
            self.status.clone(),
            resume,
            signal_rx,
        ));

        let _ = main_task.await;

        self.exit().await;

        let _ = signal_task.await;

        std::mem::take(&mut *(self.status.write().await))
    }

    fn get_context(&self) -> Context<A> {
        Context {
            self_ref: self.self_ref.upgrade().unwrap(),
            child_refs: self.child_refs.clone(),
        }
    }

    fn get_weak_context(&self) -> WeakContext<A> {
        WeakContext {
            self_ref: self.self_ref.clone(),
            child_refs: self.child_refs.clone(),
        }
    }

    fn get_supervision_context(&self) -> SupervisionContext<A> {
        SupervisionContext {
            self_ref: self.self_ref.upgrade().unwrap(),
            parent_ref: self.parent_ref.clone(),
            child_refs: self.child_refs.clone(),
        }
    }

    fn get_weak_supervision_context(&self) -> WeakSupervisionContext<A> {
        WeakSupervisionContext {
            self_ref: self.self_ref.clone(),
            parent_ref: self.parent_ref.downgrade(),
            child_refs: self.child_refs.clone(),
        }
    }

    async fn set_status(&self, status: ActorStatus<A>) {
        let mut current_status = self.status.write().await;
        *current_status = status;
    }

    async fn start(&self, args: A::Args) -> Result<A, String> {
        let ctx = self.get_context();

        let start_res = AssertUnwindSafe(A::on_start(args, &ctx))
            .catch_unwind()
            .await;

        match start_res {
            Ok(actor) => Ok(actor),
            Err(payload) => Err(panic_msg(payload)),
        }
    }

    async fn main_loop(
        mut actor: A,
        ctx: WeakSupervisionContext<A>,
        status: Arc<RwLock<ActorStatus<A>>>,
        resume: Arc<Notify>,
        mut message_rx: UnboundedReceiver<(DynMessage<A>, Option<Continuation>)>,
    ) -> (A, UnboundedReceiver<(DynMessage<A>, Option<Continuation>)>) {
        loop {
            match status.read().await.message_loop_status() {
                MessageLoopStatus::Running => {
                    if let Some((msg, k)) = message_rx.recv().await {
                        // ! Is it safe to unwrap here?
                        let ctx = ctx.upgraded().unwrap();
                        let handle_res =
                            AssertUnwindSafe(A::on_message(Immutable(&mut actor), &ctx, msg, k))
                                .catch_unwind()
                                .await;

                        if let Err(payload) = handle_res {
                            *(status.write().await) =
                                ActorStatus::PanicOnMessage(panic_msg(payload));
                        }
                    } else {
                        // Channel closed and empty
                        // Should I let each channel closed separately? or check and drop both?
                        // Anyway no way to check if it is just empty when it's not closed and the signal is alive
                        *(status.write().await) = todo!();
                        break; // Channel closed
                    }
                }
                MessageLoopStatus::Pausing => {
                    *(status.write().await) = ActorStatus::Paused;

                    // todo Pause all descendants

                    resume.notified().await; // Wait for resume signal
                }
                MessageLoopStatus::Error(err) => {
                    let ctx = ctx.upgraded().unwrap();
                    let Some(err) = err.lock().await.take() else {
                        unreachable!()
                    };

                    let recovery_res = AssertUnwindSafe(actor.on_error(&ctx, err))
                        .catch_unwind()
                        .await;

                    match recovery_res {
                        Ok(Ok(())) => {
                            // Recovery successful, continue processing messages
                            *(status.write().await) = ActorStatus::Running;
                        }
                        Ok(Err(e)) => {
                            *(status.write().await) =
                                ActorStatus::EscalatedError(Arc::new(Mutex::new(Some(e))));

                            // todo Pause all descendants

                            ctx.parent_ref.send(RawSignal::Escalation(
                                ctx.self_ref.signal_tx.clone(),
                                Escalation::Error(e.to_string()),
                            ));
                        }
                        Err(payload) => {
                            *(status.write().await) = ActorStatus::PanicOnError(panic_msg(payload));

                            ctx.parent_ref.send(RawSignal::Escalation(
                                ctx.self_ref.signal_tx.clone(),
                                Escalation::PanicOnError(panic_msg(payload)),
                            ));

                            break; // Panic can not be resumed, state considered corrupted;
                        }
                    }
                }
                MessageLoopStatus::EscalatedError(err) => {
                    resume.notified().await; // Wait for resume signal
                    // Should tern out either Running, Restarting, Terminating or Aborted
                    // Abortion will not get cautched here

                    // if it is Running should resume all the descendants
                    // If does restart also should be recursive? or only this actor?
                }

                MessageLoopStatus::Restarting => {
                    break;
                }
                MessageLoopStatus::Terminating => {
                    // Should send terminate signal to all children
                    // Wait for exit of all children
                    break; // Exit the loop on termination
                }
            }
        }

        // Run on exit right befor stopping the main loop
        actor.on_stop(&ctx, todo!()).await;

        (actor, message_rx) // Need to be re used
    }

    async fn signal_loop(
        abort_handle: tokio::task::AbortHandle,
        ctx: WeakContext<A>,
        status: Arc<RwLock<ActorStatus<A>>>,
        resume: Arc<Notify>,
        mut signal_rx: UnboundedReceiver<RawSignal>,
    ) -> UnboundedReceiver<RawSignal> {
        while let Some(signal) = signal_rx.recv().await {
            match signal {
                RawSignal::Escalation(subordinate, escalation) => {
                    let mut status = status.write().await;
                    // ! What if it is already on Error or Escalation etc?
                }
                RawSignal::Pause(k) => {
                    let mut status = status.write().await;
                    match &*status {
                        ActorStatus::Starting | ActorStatus::Running => {
                            *status = ActorStatus::Pausing;
                        }
                        _ => {
                            #[cfg(debug_assertions)]
                            tracing::trace!("Pause signal in {status:?} does not have effect",);
                        }
                    }
                }
                RawSignal::Resume(k) => {
                    let mut status = status.write().await;
                    match &*status {
                        ActorStatus::Pausing
                        | ActorStatus::Paused
                        | ActorStatus::Error(_)
                        | ActorStatus::EscalatedError(_) => {
                            *status = ActorStatus::Running;
                            resume.notify_one();
                        }
                        _ => {
                            #[cfg(debug_assertions)]
                            tracing::trace!("Resume signal in {status:?} does not have effect",);
                        }
                    }
                }
                RawSignal::Restart(k) => {
                    let mut status = status.write().await;
                    match &*status {
                        ActorStatus::Pausing
                        | ActorStatus::Paused
                        | ActorStatus::Error(_)
                        | ActorStatus::EscalatedError(_)
                        | ActorStatus::PanicOnMessage(_)
                        | ActorStatus::PanicOnError(_)
                        | ActorStatus::PanicOnEscalation(_) => {
                            *status = ActorStatus::Restarting;
                            // todo should recursivly restart all children
                        }
                        _ => {
                            #[cfg(debug_assertions)]
                            tracing::error!("Restart signal in {status:?} does not have effect",);
                        }
                    }
                    break; // Take no more signals
                }
                RawSignal::Terminate(k) => {
                    // ! Termination logic must not fail and has to call notify
                    // Actual termination initiated on completion of current message processing
                    *(status.write().await) = ActorStatus::Terminating;
                    break; // Take no more signals
                }
                RawSignal::Abort(k) => {
                    abort_handle.abort();
                    *(status.write().await) = ActorStatus::Aborted;
                    // todo Directly send abort signal to all children
                    break; // Take no more signals
                }
            }
        }

        // ! More than two signal could make issue, e.g. Restart and Terminate => Terminate
        signal_rx
    }

    async fn exit(&self) -> ActorStatus<A> {
        let status = self.status.write().await;
        match &*status {
            ActorStatus::Terminating => {}
            ActorStatus::Dropping => {}
        }
    }
}
