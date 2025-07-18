use std::{any::type_name, panic::AssertUnwindSafe, sync::Arc};

use futures::{FutureExt, future::join_all};
use tokio::{
    select,
    sync::{
        Notify,
        mpsc::{UnboundedReceiver, error::TryRecvError},
    },
};

use crate::{
    actor::Actor,
    actor_ref::{SupervisionRef, WeakActorRef, WeakSupervisionRef},
    base::{Immutable, panic_msg},
    context::{Context, SupervisionContext},
    message::{Continuation, DynMessage, Escalation, RawSignal, Signal},
};

pub(crate) struct ActorConfig<A: Actor> {
    pub(crate) self_ref: WeakActorRef<A>, // Self reference
    pub(crate) self_supervision_ref: SupervisionRef, // Self supervision reference
    pub(crate) parent_ref: SupervisionRef, // Parent supervision reference
    pub(crate) child_refs: Vec<WeakSupervisionRef>, // Children of this actor

    pub(crate) msg_rx: UnboundedReceiver<(DynMessage<A>, Option<Continuation>)>, // Message receiver
    pub(crate) sig_rx: UnboundedReceiver<RawSignal>,                             // Signal receiver

    pub(crate) args: A::Args, // Arguments for actor initialization

    pub(crate) mb_restart_k: Option<Arc<Notify>>, // Optional continuation for restart signal
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
    Exit,
}

// Continuation of an actor
enum Cont {
    Process,

    Pause(Option<Arc<Notify>>),
    WaitSignal,
    Resume(Option<Arc<Notify>>),

    Escalation(Option<SupervisionRef>, Option<Escalation>),

    Panic(Option<Escalation>),
    Restart(Option<Arc<Notify>>),

    Drop,
    Terminate(Option<Arc<Notify>>),
}

pub enum ExitCode {
    Dropped,
    Terminated,
}

// Implementations

impl<A> ActorConfig<A>
where
    A: Actor,
{
    pub(crate) fn new(
        self_ref: WeakActorRef<A>,
        self_supervision_ref: SupervisionRef,
        parent_ref: SupervisionRef,
        args: A::Args,
        sig_rx: UnboundedReceiver<RawSignal>,
        msg_rx: UnboundedReceiver<(DynMessage<A>, Option<Continuation>)>,
    ) -> Self {
        let child_refs = Vec::new();
        let mb_restart_k = None;

        Self {
            self_ref,
            self_supervision_ref,
            parent_ref,
            child_refs,
            sig_rx,
            msg_rx,
            args,
            mb_restart_k,
        }
    }

    pub(crate) async fn exec(self) {
        let mut mb_config = Some(self);

        loop {
            match mb_config {
                Some(config) => mb_config = config.exec_impl().await,
                None => break,
            }
        }
    }

    async fn exec_impl(self) -> Option<Self> {
        let mb_inst = self.init_instance().await;

        let mut inst = match mb_inst {
            Ok(inst) => inst,
            Err((config, e)) => {
                if let Err(_) = config
                    .parent_ref
                    .escalate(config.self_supervision_ref.clone(), e)
                {
                    return None; // Terminate if escalation failed
                }

                return config.wait_signal().await;
            }
        };

        if inst.inner.config.mb_restart_k.is_some() {
            inst.inner.config.mb_restart_k.take().unwrap().notify_one();
        }

        let mut lifecycle = Lifecycle::Running(inst);

        loop {
            match lifecycle {
                Lifecycle::Running(inst) => {
                    lifecycle = inst.run().await;
                }
                Lifecycle::Restarting(config) => return Some(config),
                Lifecycle::Exit => return None,
            }
        }
    }

    async fn init_instance(self) -> Result<ActorInstance<A>, (Self, Escalation)> {
        Ok(ActorInstance {
            k: Cont::Process,
            inner: ActorInstanceImpl::init(self).await?,
        })
    }

    async fn wait_signal(mut self) -> Option<Self> {
        let mb_sig = self.sig_rx.recv().await;
        let Some(sig) = mb_sig else {
            return None;
        };

        match sig {
            RawSignal::Restart(k) => {
                self.mb_restart_k = k;
                Some(self)
            }
            _ => None,
        }
    }

    fn ctx(&mut self) -> Context<A> {
        Context {
            self_ref: self.self_ref.clone().upgrade().unwrap(), // ! Safe?
            child_refs: &mut self.child_refs,
            self_supervision_ref: self.self_supervision_ref.clone(),
        }
    }

    fn supervision_ctx(&mut self) -> SupervisionContext<A> {
        SupervisionContext {
            self_ref: self.self_ref.clone().upgrade().unwrap(), // ! Safe?
            child_refs: &mut self.child_refs,
            self_supervision_ref: self.self_supervision_ref.clone(),
        }
    }

    fn ctx_args(&mut self) -> (Context<A>, &A::Args) {
        (
            Context {
                self_ref: self.self_ref.clone().upgrade().unwrap(), // ! Safe?
                child_refs: &mut self.child_refs,
                self_supervision_ref: self.self_supervision_ref.clone(),
            },
            &self.args,
        )
    }
}

impl<A> ActorInstance<A>
where
    A: Actor,
{
    async fn run(mut self) -> Lifecycle<A> {
        loop {
            self.k = match &mut self.k {
                Cont::Process => self.inner.process().await,
                Cont::Pause(k) => self.inner.pause(k).await,
                Cont::WaitSignal => self.inner.wait_signal().await,
                Cont::Resume(k) => self.inner.resume(k).await,

                Cont::Escalation(s @ Some(_), e @ Some(_)) => self.inner.supervise(s, e).await,
                Cont::Escalation(_, _) => unreachable!(),

                Cont::Panic(e @ Some(_)) => self.inner.escalate(e).await,
                Cont::Panic(None) => unreachable!(),

                Cont::Restart(k) => return self.inner.restart(k).await,
                Cont::Drop => return self.inner.drop().await,
                Cont::Terminate(k) => return self.inner.terminate(k).await,
            };
        }
    }
}

impl<A> ActorInstanceImpl<A>
where
    A: Actor,
{
    async fn init(mut config: ActorConfig<A>) -> Result<Self, (ActorConfig<A>, Escalation)> {
        let (ctx, args) = config.ctx_args();

        let init_res = AssertUnwindSafe(A::init(ctx, args)).catch_unwind().await;

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
                    Ok(sig) => return self.process_sig(sig).await,
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
                            return self.process_sig(sig).await;
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

    async fn process_sig(&mut self, sig: RawSignal) -> Cont {
        match sig {
            RawSignal::Escalation(s, e) => Cont::Escalation(Some(s), Some(e)),
            RawSignal::Pause(k) => Cont::Pause(k),
            RawSignal::Resume(k) => Cont::Resume(k),
            RawSignal::Restart(k) => Cont::Restart(k),
            RawSignal::Terminate(k) => Cont::Terminate(k),
        }
    }

    async fn process_msg(
        &mut self,
        (msg, k): (DynMessage<A>, Option<Continuation>),
    ) -> Option<Cont> {
        let ro_self = Immutable(&mut self.state);
        let ctx = self.config.ctx();

        let res = AssertUnwindSafe(A::on_message(ro_self, ctx, msg, k))
            .catch_unwind()
            .await;

        if let Err(e) = res {
            return Some(Cont::Panic(Some(Escalation::PanicOnMessage(panic_msg(e)))));
        }

        None
    }

    async fn pause(&mut self, k: &mut Option<Arc<Notify>>) -> Cont {
        self.signal_children(Signal::Pause, k).await;

        Cont::WaitSignal
    }

    async fn wait_signal(&mut self) -> Cont {
        let mb_sig = self.config.sig_rx.recv().await;
        let Some(sig) = mb_sig else {
            return Cont::Drop;
        };

        self.process_sig(sig).await
    }

    async fn resume(&mut self, k: &mut Option<Arc<Notify>>) -> Cont {
        self.signal_children(Signal::Resume, k).await;

        Cont::Process
    }

    async fn supervise(
        &mut self,
        s: &mut Option<SupervisionRef>,
        e: &mut Option<Escalation>,
    ) -> Cont {
        let s = s.take().unwrap();
        let e = e.take().unwrap();

        let supervision_ctx = self.config.supervision_ctx();

        let res = AssertUnwindSafe(self.state.supervise(supervision_ctx, s, e))
            .catch_unwind()
            .await;

        if let Err(e) = res {
            return Cont::Panic(Some(Escalation::PanicOnSupervision(panic_msg(e))));
        }

        Cont::Process
    }

    async fn escalate(&mut self, e: &mut Option<Escalation>) -> Cont {
        let e = e.take().unwrap();

        self.signal_children(Signal::Pause, &mut None).await;

        let res = self
            .config
            .parent_ref
            .escalate(self.config.self_supervision_ref.clone(), e);

        if let Err(_) = res {
            return Cont::Terminate(None);
        }

        Cont::WaitSignal
    }

    async fn restart(mut self, k: &mut Option<Arc<Notify>>) -> Lifecycle<A> {
        self.signal_children(Signal::Restart, &mut None).await;

        self.config.mb_restart_k = k.take();

        let res = AssertUnwindSafe(A::on_restart(&mut self.state, self.config.ctx()))
            .catch_unwind()
            .await;

        if let Err(e) = res {
            #[cfg(feature = "tracing")]
            tracing::warn!("{} on_restart panic: {}", type_name::<A>(), panic_msg(e));
        }

        Lifecycle::Restarting(self.config)
    }

    async fn drop(&mut self) -> Lifecycle<A> {
        // ! Actually this should be no-op, since there has to be no children, but need further investigation
        self.signal_children(Signal::Terminate, &mut None).await;

        let res = AssertUnwindSafe(A::on_exit(
            &mut self.state,
            self.config.ctx(),
            ExitCode::Dropped,
        ))
        .catch_unwind()
        .await;

        if let Err(_e) = res {
            #[cfg(feature = "tracing")]
            tracing::warn!("{} on_exit panic: {}", type_name::<A>(), panic_msg(_e));
        }

        Lifecycle::Exit
    }

    async fn terminate(&mut self, k: &mut Option<Arc<Notify>>) -> Lifecycle<A> {
        self.signal_children(Signal::Terminate, k).await;

        let res = AssertUnwindSafe(A::on_exit(
            &mut self.state,
            self.config.ctx(),
            ExitCode::Dropped,
        ))
        .catch_unwind()
        .await;

        if let Err(_e) = res {
            #[cfg(feature = "tracing")]
            tracing::warn!("{} on_exit panic: {}", type_name::<A>(), panic_msg(e));
        }

        Lifecycle::Exit
    }

    async fn signal_children(&mut self, signal: Signal, k: &mut Option<Arc<Notify>>) {
        let alive_child_refs = self.alive_child_refs();

        let mut pause_children = Vec::new();
        for child_ref in &alive_child_refs {
            pause_children.push(child_ref.signal(signal).into_future());
        }

        // Wait for all of them
        join_all(pause_children.into_iter()).await;

        self.config.child_refs = alive_child_refs.iter().map(|c| c.downgrade()).collect();

        if let Some(k) = k {
            k.notify_one();
        }
    }

    fn alive_child_refs(&self) -> Vec<SupervisionRef> {
        self.config
            .child_refs
            .iter()
            .filter_map(|c| c.upgrade())
            .collect()
    }
}
