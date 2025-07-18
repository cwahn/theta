use std::{mem::replace, panic::AssertUnwindSafe};

use futures::FutureExt;
use tokio::{
    select,
    sync::mpsc::{UnboundedReceiver, error::TryRecvError},
};

use crate::{
    actor::Actor,
    actor_ref::{SupervisionRef, WeakActorRef, WeakSupervisionRef},
    base::panic_msg,
    context::Context,
    message::{Continuation, DynMessage},
    signal::{Escalation, RawSignal},
};

pub(crate) struct ActorConfig<A: Actor> {
    pub(crate) self_ref: WeakActorRef<A>, // Self reference
    pub(crate) self_supervision_ref: SupervisionRef, // Self supervision reference
    pub(crate) parent_ref: SupervisionRef, // Parent supervision reference
    pub(crate) child_refs: Vec<WeakSupervisionRef>, // Children of this actor

    pub(crate) msg_rx: UnboundedReceiver<(DynMessage<A>, Option<Continuation>)>, // Message receiver
    pub(crate) sig_rx: UnboundedReceiver<RawSignal>,                             // Signal receiver

    pub(crate) args: A::Args, // Arguments for actor initialization
}

struct ActorInstance<A: Actor> {
    k: Cont,
    inner: ActorInstanceImpl<A>,
}

struct ActorInstanceImpl<A: Actor> {
    state: A,
    config: ActorConfig<A>,
}

enum Lifecycle<A: Actor> {
    Running(ActorInstance<A>),
    Restarting(ActorConfig<A>),
    Exit(ExitCode),
}

// Continuation of an actor
enum Cont {
    Process,
    Pause,
    Escalate(Option<Escalation>),
    Restart,
    Drop,
    Terminate,
}

enum ExitCode {
    Dropped,
    Terminated,
}

// Implementations

impl<A> ActorConfig<A>
where
    A: Actor,
{
    pub async fn exec(self) -> ExitCode {
        let mut mb_config = Ok(self);

        loop {
            match mb_config {
                Ok(config) => mb_config = config.exec_impl().await,
                Err(exit_code) => return exit_code,
            }
        }
    }

    pub async fn exec_impl(self) -> Result<Self, ExitCode> {
        let mb_inst = self.init_instance().await;

        let inst = match mb_inst {
            Ok(inst) => inst,
            Err((config, e)) => {
                if let Err(_) = config.parent_ref.escalate(e) {
                    return Err(ExitCode::Terminated);
                }

                return config.process_sig().await;
            }
        };

        let mut lifecycle = Lifecycle::Running(inst);

        loop {
            match lifecycle {
                Lifecycle::Running(inst) => {
                    lifecycle = inst.run().await;
                }
                Lifecycle::Restarting(config) => return Ok(config),
                Lifecycle::Exit(exit_code) => return Err(exit_code),
            }
        }
    }

    async fn init_instance(self) -> Result<ActorInstance<A>, (Self, Escalation)> {
        Ok(ActorInstance {
            k: Cont::Process,
            inner: ActorInstanceImpl::init(self).await?,
        })
    }

    async fn process_sig(mut self) -> Result<Self, ExitCode> {
        let mb_sig = self.sig_rx.recv().await;
        let Some(sig) = mb_sig else {
            return Err(ExitCode::Terminated);
        };

        match sig {
            RawSignal::Restart(k) => Ok(self),
            _ => Err(ExitCode::Terminated),
        }
    }
}

impl<A> ActorInstance<A>
where
    A: Actor,
{
    pub async fn run(mut self) -> Lifecycle<A> {
        loop {
            self.k = match &mut self.k {
                Cont::Process => self.inner.process().await,
                Cont::Pause => self.inner.pause().await,
                Cont::Escalate(e @ Some(_)) => self.inner.escalate(replace(e, None).unwrap()).await,
                Cont::Escalate(None) => unreachable!(),
                Cont::Restart => return Lifecycle::Restarting(self.inner.config),
                Cont::Drop => return self.inner.drop().await,
                Cont::Terminate => return self.inner.terminate().await,
            };
        }
    }
}

impl<A> ActorInstanceImpl<A>
where
    A: Actor,
{
    async fn init(config: ActorConfig<A>) -> Result<Self, (ActorConfig<A>, Escalation)> {
        let mut child_refs = Vec::new();

        let ctx = Context {
            self_ref: config.self_ref.clone().upgrade().unwrap(), // ! Safe?
            child_refs: &mut child_refs,
            self_supervision_ref: config.self_supervision_ref.clone(), // Self supervision reference
        };

        let init_res = AssertUnwindSafe(A::init(ctx, &config.args))
            .catch_unwind()
            .await;

        let state = match init_res {
            Ok(state) => state,
            Err(e) => {
                return Err((config, Escalation::PanicOnStart(panic_msg(e))));
            }
        };

        Ok(ActorInstanceImpl { config, state })
    }

    async fn process(&mut self) -> Cont {
        loop {
            let is_closed = loop {
                match self.config.sig_rx.try_recv() {
                    Ok(sig) => {
                        if let Some(k) = self.process_sig(sig).await {
                            return k;
                        }
                    }
                    Err(TryRecvError::Empty) => break false,
                    Err(TryRecvError::Disconnected) => break true,
                }
            };

            if is_closed {
                return Cont::Drop;
            }

            match self.config.msg_rx.try_recv() {
                Ok(msg_k) => {
                    if let Some(k) = self.process_msg(msg_k).await {
                        return k;
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    return Cont::Drop;
                }
                Err(TryRecvError::Empty) => {
                    select! {
                        mb_sig = self.config.sig_rx.recv() => if let Some(sig) = mb_sig {
                            if let Some(k) = self.process_sig(sig).await {
                                return k;
                            }
                        } else {
                            return Cont::Drop;
                        },
                        mb_msg = self.config.msg_rx.recv() => if let Some(msg_k) = mb_msg {
                            if let Some(k) = self.process_msg(msg_k).await {
                                return k;
                            }
                        } else {
                            return Cont::Drop;
                        },
                    }
                }
            }
        }
    }

    async fn pause(&mut self) -> Cont {
        todo!()
    }

    async fn escalate(&mut self, e: Escalation) -> Cont {
        todo!()
    }

    async fn drop(&mut self) -> Lifecycle<A> {
        todo!()
    }

    async fn terminate(&mut self) -> Lifecycle<A> {
        todo!()
    }

    async fn process_sig(&mut self, sig: RawSignal) -> Option<Cont> {
        todo!()
    }

    async fn process_msg(
        &mut self,
        (msg, k): (DynMessage<A>, Option<Continuation>),
    ) -> Option<Cont> {
        todo!()
    }
}
