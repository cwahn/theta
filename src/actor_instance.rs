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
    actor_ref::{ActorHdl, WeakActorRef, WeakSignalHdl},
    base::{Immutable, panic_msg},
    context::{Context, SuperContext},
    message::{Continuation, DynMessage, Escalation, RawSignal, Signal},
};

pub(crate) struct ActorConfig<A: Actor> {
    pub(crate) this: WeakActorRef<A>,          // Self reference
    pub(crate) this_hdl: ActorHdl,             // Self supervision reference
    pub(crate) parent_hdl: ActorHdl,           // Parent supervision reference
    pub(crate) child_hdls: Vec<WeakSignalHdl>, // Children of this actor

    pub(crate) msg_rx: UnboundedReceiver<(DynMessage<A>, Option<Continuation>)>, // Message receiver
    pub(crate) sig_rx: UnboundedReceiver<RawSignal>,                             // Signal receiver

    pub(crate) args: A::Args, // Arguments for actor initialization

    pub(crate) mb_restart_k: Option<Arc<Notify>>, // Optional continuation for restart signal
}

struct ActorInstance<A: Actor> {
    k: Cont,
    state: ActorState<A>,
}

struct ActorState<A: Actor> {
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

    Escalation(Option<ActorHdl>, Option<Escalation>),

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
        this: WeakActorRef<A>,
        this_hdl: ActorHdl,
        parent_hdl: ActorHdl,
        args: A::Args,
        sig_rx: UnboundedReceiver<RawSignal>,
        msg_rx: UnboundedReceiver<(DynMessage<A>, Option<Continuation>)>,
    ) -> Self {
        let child_hdls = Vec::new();
        let mb_restart_k = None;

        Self {
            this,
            this_hdl,
            parent_hdl,
            child_hdls,
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
                if let Err(_) = config.parent_hdl.escalate(config.this_hdl.clone(), e) {
                    return None; // Terminate if escalation failed
                }

                return config.wait_signal().await;
            }
        };

        if inst.state.config.mb_restart_k.is_some() {
            inst.state.config.mb_restart_k.take().unwrap().notify_one();
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
            state: ActorState::init(self).await?,
        })
    }

    async fn wait_signal(mut self) -> Option<Self> {
        let Some(sig) = self.sig_rx.recv().await else {
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
            this: self.this.clone().upgrade().unwrap(), // ! Safe?
            child_hdls: &mut self.child_hdls,
            this_hdl: self.this_hdl.clone(),
        }
    }

    fn supervision_ctx(&mut self) -> SuperContext<A> {
        SuperContext {
            this: self.this.clone().upgrade().unwrap(), // ! Safe?
            child_hdls: &mut self.child_hdls,
            this_hdl: self.this_hdl.clone(),
        }
    }

    fn ctx_args(&mut self) -> (Context<A>, &A::Args) {
        (
            Context {
                this: self.this.clone().upgrade().unwrap(), // ! Safe?
                child_hdls: &mut self.child_hdls,
                this_hdl: self.this_hdl.clone(),
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
                Cont::Process => self.state.process().await,
                Cont::Pause(k) => self.state.pause(k).await,
                Cont::WaitSignal => self.state.wait_signal().await,
                Cont::Resume(k) => self.state.resume(k).await,

                Cont::Escalation(s @ Some(_), e @ Some(_)) => self.state.supervise(s, e).await,
                Cont::Escalation(_, _) => unreachable!(),

                Cont::Panic(e @ Some(_)) => self.state.escalate(e).await,
                Cont::Panic(None) => unreachable!(),

                Cont::Restart(k) => return self.state.restart(k).await,
                Cont::Drop => return self.state.drop().await,
                Cont::Terminate(k) => return self.state.terminate(k).await,
            };
        }
    }
}

impl<A> ActorState<A>
where
    A: Actor,
{
    async fn init(mut config: ActorConfig<A>) -> Result<Self, (ActorConfig<A>, Escalation)> {
        let (ctx, args) = config.ctx_args();

        let init_res = AssertUnwindSafe(A::init(ctx, args)).catch_unwind().await;

        let state = match init_res {
            Ok(state) => state,
            Err(e) => {
                return Err((config, Escalation::PanicOnInit(panic_msg(e))));
            }
        };

        Ok(ActorState { config, state })
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

        let res = AssertUnwindSafe(A::process(ro_self, ctx, msg, k))
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
        let Some(sig) = self.config.sig_rx.recv().await else {
            return Cont::Drop;
        };

        self.process_sig(sig).await
    }

    async fn resume(&mut self, k: &mut Option<Arc<Notify>>) -> Cont {
        self.signal_children(Signal::Resume, k).await;

        Cont::Process
    }

    async fn supervise(&mut self, s: &mut Option<ActorHdl>, e: &mut Option<Escalation>) -> Cont {
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
            .parent_hdl
            .escalate(self.config.this_hdl.clone(), e);

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

    async fn signal_children(&mut self, sig: Signal, k: &mut Option<Arc<Notify>>) {
        let active_child_hdls = self.active_child_hdls();

        let mut pause_ks = Vec::new();
        for child_hdl in &active_child_hdls {
            pause_ks.push(child_hdl.signal(sig).into_future());
        }

        // Wait for all of them
        join_all(pause_ks.into_iter()).await;

        self.config.child_hdls = active_child_hdls.iter().map(|c| c.downgrade()).collect();

        if let Some(k) = k {
            k.notify_one();
        }
    }

    fn active_child_hdls(&self) -> Vec<ActorHdl> {
        self.config
            .child_hdls
            .iter()
            .filter_map(|c| c.upgrade())
            .collect()
    }
}
