use std::{
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex},
};

use futures::{FutureExt, future::join_all};
use log::error;
use tokio::{select, sync::Notify};

use crate::{
    actor::{Actor, ActorArgs, ExitCode},
    actor_ref::{ActorHdl, WeakActorHdl, WeakActorRef},
    base::panic_msg,
    context::Context,
    message::{Continuation, Escalation, InternalSignal, MsgRx, RawSignal, SigRx},
};

#[cfg(feature = "monitor")]
use crate::monitor::{AnyUpdateTx, Monitor, Update, UpdateTx};

/// Configuration and runtime resources for an actor instance.
pub(crate) struct ActorConfig<A: Actor, Args: ActorArgs<Actor = A>> {
    pub(crate) this: WeakActorRef<A>, // Self reference

    pub(crate) parent_hdl: ActorHdl, // Parent handle
    pub(crate) this_hdl: ActorHdl,   // Self handle
    pub(crate) child_hdls: Arc<Mutex<Vec<WeakActorHdl>>>, // Children of this actor

    pub(crate) sig_rx: SigRx,
    pub(crate) msg_rx: MsgRx<A>,

    #[cfg(feature = "monitor")]
    pub(crate) monitor: Monitor<A>, // Monitor for this actor

    pub(crate) args: Args, // Arguments for actor initialization
    pub(crate) mb_restart_k: Option<Arc<Notify>>, // Optional continuation for restart signal
}

/// Active actor instance with its execution continuation.
pub(crate) struct ActorInst<A: Actor, Args: ActorArgs<Actor = A>> {
    k: Cont,
    state: ActorState<A, Args>,
}

/// Actor state container with configuration and hash.
pub(crate) struct ActorState<A: Actor, Args: ActorArgs<Actor = A>> {
    state: A,
    #[cfg(feature = "monitor")]
    hash: u64,
    config: ActorConfig<A, Args>,
}

/// Actor lifecycle states for runtime management.
pub(crate) enum Lifecycle<A: Actor, Args: ActorArgs<Actor = A>> {
    Running(ActorInst<A, Args>),
    Restarting(ActorConfig<A, Args>),
    Exit,
}

/// Execution continuation states for actor processing.
pub(crate) enum Cont {
    Process,

    Pause(Option<Arc<Notify>>),
    WaitSignal,
    Resume(Option<Arc<Notify>>),

    Supervise(ActorHdl, Escalation),
    CleanupChildren,

    Panic(Escalation),
    Restart(Option<Arc<Notify>>),

    Drop,
    Terminate(Option<Arc<Notify>>),
}

// Implementations

impl<A, Args> ActorConfig<A, Args>
where
    A: Actor,
    Args: ActorArgs<Actor = A>,
{
    pub(crate) fn new(
        this: WeakActorRef<A>,
        parent_hdl: ActorHdl,
        this_hdl: ActorHdl,
        sig_rx: SigRx,
        msg_rx: MsgRx<A>,
        args: Args,
    ) -> Self {
        let child_hdls = Arc::new(Mutex::new(Vec::new()));
        let mb_restart_k = None;
        #[cfg(feature = "monitor")]
        let monitor = Monitor::default();

        Self {
            this,
            parent_hdl,
            this_hdl,
            child_hdls,
            sig_rx,
            msg_rx,
            #[cfg(feature = "monitor")]
            monitor,
            args,
            mb_restart_k,
        }
    }

    pub(crate) async fn exec(self) {
        let mut mb_config = Some(self);

        while let Some(config) = mb_config {
            mb_config = config.exec_impl().await;
        }
    }

    async fn exec_impl(self) -> Option<Self> {
        let mb_inst = self.init_instance().await;

        let inst = match mb_inst {
            Err((config, e)) => {
                config
                    .parent_hdl
                    .escalate(config.this_hdl.clone(), e)
                    .expect("Escalation should not fail");

                return config.wait_signal().await;
            }
            Ok(inst) => inst,
        };

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

    async fn init_instance(self) -> Result<ActorInst<A, Args>, (Self, Escalation)> {
        Ok(ActorInst {
            k: Cont::Process,
            state: ActorState::init(self).await?,
        })
    }

    async fn wait_signal(mut self) -> Option<Self> {
        let sig = self.sig_rx.recv().await.unwrap();

        match sig {
            RawSignal::Restart(k) => {
                self.mb_restart_k = k;
                Some(self)
            }
            _ => None,
        }
    }

    fn ctx_cfg(&mut self) -> (Context<A>, &Args) {
        (
            Context {
                this: self.this.clone(),
                child_hdls: self.child_hdls.clone(),
                this_hdl: self.this_hdl.clone(),
            },
            &self.args,
        )
    }

    fn ctx(&mut self) -> Context<A> {
        Context {
            this: self.this.clone(),
            child_hdls: self.child_hdls.clone(),
            this_hdl: self.this_hdl.clone(),
        }
    }
}

impl<A, Args> ActorInst<A, Args>
where
    A: Actor,
    Args: ActorArgs<Actor = A>,
{
    async fn run(mut self) -> Lifecycle<A, Args> {
        loop {
            #[cfg(feature = "monitor")]
            self.state
                .config
                .monitor
                .update(Update::Status((&self.k).into()));

            self.k = match self.k {
                Cont::Process => self.state.process().await,

                Cont::Pause(k) => self.state.pause(k).await,
                Cont::WaitSignal => self.state.wait_signal().await,
                Cont::Resume(k) => self.state.resume(k).await,

                Cont::Supervise(c, e) => self.state.supervise(c, e).await,
                Cont::CleanupChildren => self.state.cleanup_children().await,

                Cont::Panic(e) => self.state.escalate(e).await,
                Cont::Restart(k) => return self.state.restart(k).await,

                Cont::Drop => return self.state.drop().await,
                Cont::Terminate(k) => return self.state.terminate(k).await,
            };
        }
    }
}

impl<A, Args> ActorState<A, Args>
where
    A: Actor,
    Args: ActorArgs<Actor = A>,
{
    async fn init(
        mut config: ActorConfig<A, Args>,
    ) -> Result<Self, (ActorConfig<A, Args>, Escalation)> {
        let (ctx, cfg) = config.ctx_cfg();

        let init_res = Args::initialize(ctx, cfg).catch_unwind().await;

        if let Some(k) = config.mb_restart_k.take() {
            k.notify_one()
        }

        let state = match init_res {
            Err(e) => {
                config.child_hdls.clear_poison();
                return Err((config, Escalation::Initialize(panic_msg(e))));
            }
            Ok(state) => state,
        };

        #[cfg(feature = "monitor")]
        let hash_code = state.hash_code();

        Ok(ActorState {
            state,
            #[cfg(feature = "monitor")]
            hash: hash_code,
            config,
        })
    }

    async fn process(&mut self) -> Cont {
        loop {
            select! {
                biased;
                mb_sig = self.config.sig_rx.recv() => match self.process_sig(mb_sig.unwrap()) {
                    None => continue, // Continue processing signals
                    Some(k) => return k,
                },
                mb_msg_k = self.config.msg_rx.recv() => match mb_msg_k {
                    None => {
                        // If self, parent, and global left, drop
                        if self.config.sig_rx.sender_count() == 3 {
                            return Cont::Drop;
                        }
                        return Cont::WaitSignal;
                    },
                    Some(msg_k) => if let Some(k) = self.process_msg(msg_k).await {
                        return k;
                    },
                },
            }
        }
    }

    #[cfg(feature = "monitor")]
    fn add_monitor(&mut self, any_tx: AnyUpdateTx) {
        let Ok(tx) = any_tx.downcast::<UpdateTx<A>>() else {
            return error!("{} received invalid monitor", std::any::type_name::<A>(),);
        };

        if let Err(e) = tx.send(Update::State(self.state.state_view())) {
            return error!("Failed to send initial state update to monitor: {e}");
        }

        self.config.monitor.add_monitor(*tx);
    }

    async fn pause(&mut self, k: Option<Arc<Notify>>) -> Cont {
        self.signal_children(InternalSignal::Pause, k).await;

        Cont::WaitSignal
    }

    async fn wait_signal(&mut self) -> Cont {
        loop {
            let sig = self.config.sig_rx.recv().await.unwrap();
            // Monitor does not count in this context
            match self.process_sig(sig) {
                None => continue,
                Some(k) => return k,
            }
        }
    }

    async fn resume(&mut self, k: Option<Arc<Notify>>) -> Cont {
        self.signal_children(InternalSignal::Resume, k).await;

        Cont::Process
    }

    async fn supervise(&mut self, child_hdl: ActorHdl, escalation: Escalation) -> Cont {
        let res = AssertUnwindSafe(self.state.supervise(escalation))
            .catch_unwind()
            .await;

        match res {
            Err(e) => {
                self.config.child_hdls.clear_poison();

                return Cont::Panic(Escalation::Supervise(panic_msg(e)));
            }
            Ok((one, rest)) => {
                let alive_hdls: Vec<_> = self
                    .config
                    .child_hdls
                    .lock()
                    .unwrap()
                    .iter()
                    .filter_map(|c| c.upgrade())
                    .collect();

                let sig_ks = alive_hdls.iter().filter_map(|hdl| {
                    if hdl == &child_hdl {
                        Some(hdl.signal(one.into()).into_future())
                    } else {
                        rest.map(|r| hdl.signal(r.into()).into_future())
                    }
                });

                join_all(sig_ks).await;
            }
        }

        Cont::Process
    }

    async fn cleanup_children(&mut self) -> Cont {
        self.config
            .child_hdls
            .lock()
            .unwrap()
            .retain(|hdl| hdl.0.strong_count() > 0);

        Cont::Process
    }

    async fn escalate(&mut self, e: Escalation) -> Cont {
        self.signal_children(InternalSignal::Pause, None).await;

        let res = self
            .config
            .parent_hdl
            .escalate(self.config.this_hdl.clone(), e);

        if res.is_err() {
            return Cont::Terminate(None);
        }

        Cont::WaitSignal
    }

    async fn restart(mut self, k: Option<Arc<Notify>>) -> Lifecycle<A, Args> {
        self.config.mb_restart_k = k;

        self.signal_children(InternalSignal::Terminate, None).await;

        let res = AssertUnwindSafe(A::on_restart(&mut self.state))
            .catch_unwind()
            .await;

        if let Err(e) = res {
            self.config.child_hdls.clear_poison();

            error!(
                "{} on_restart panic: {}",
                std::any::type_name::<A>(),
                panic_msg(e)
            );
        }

        Lifecycle::Restarting(self.config)
    }

    async fn drop(&mut self) -> Lifecycle<A, Args> {
        for sig in self.config.sig_rx.drain() {
            match sig {
                RawSignal::Pause(k)
                | RawSignal::Resume(k)
                | RawSignal::Restart(k)
                | RawSignal::Terminate(k) => {
                    if let Some(k) = k {
                        k.notify_one()
                    }
                }
                s => {
                    error!(
                        "{} received unexpected signal while dropping: {s:#?}",
                        std::any::type_name::<A>()
                    );
                }
            }
        }

        let res = AssertUnwindSafe(A::on_exit(&mut self.state, ExitCode::Dropped))
            .catch_unwind()
            .await;

        if let Err(e) = res {
            self.config.child_hdls.clear_poison();

            error!(
                "{} on_exit panic: {}",
                std::any::type_name::<A>(),
                panic_msg(e)
            );
        }

        self.config
            .parent_hdl
            .raw_send(RawSignal::ChildDropped)
            .expect("parent lives longer than child");

        Lifecycle::Exit
    }

    async fn terminate(&mut self, k: Option<Arc<Notify>>) -> Lifecycle<A, Args> {
        self.signal_children(InternalSignal::Terminate, k).await;

        let res = AssertUnwindSafe(A::on_exit(&mut self.state, ExitCode::Dropped))
            .catch_unwind()
            .await;

        if let Err(e) = res {
            self.config.child_hdls.clear_poison();

            error!(
                "{} on_exit panic: {}",
                std::any::type_name::<A>(),
                panic_msg(e)
            );
        }

        Lifecycle::Exit
    }

    fn process_sig(&mut self, sig: RawSignal) -> Option<Cont> {
        match sig {
            #[cfg(feature = "monitor")]
            RawSignal::Monitor(t) => {
                self.add_monitor(t);
                None
            }

            RawSignal::Escalation(c, e) => Some(Cont::Supervise(c, e)),
            RawSignal::ChildDropped => Some(Cont::CleanupChildren),

            RawSignal::Pause(k) => Some(Cont::Pause(k)),
            RawSignal::Resume(k) => Some(Cont::Resume(k)),
            RawSignal::Restart(k) => Some(Cont::Restart(k)),
            RawSignal::Terminate(k) => Some(Cont::Terminate(k)),
        }

        // This is the place where monitor can access
    }

    async fn process_msg(&mut self, (msg, k): (A::Msg, Continuation)) -> Option<Cont> {
        let ctx = self.config.ctx();
        let res = AssertUnwindSafe(self.state.process_msg(ctx, msg, k))
            .catch_unwind()
            .await;

        if let Err(e) = res {
            self.config.child_hdls.clear_poison();
            return Some(Cont::Panic(Escalation::ProcessMsg(panic_msg(e))));
        }

        #[cfg(feature = "monitor")]
        if self.config.monitor.is_monitor() {
            let new_hash = self.state.hash_code();

            if new_hash != self.hash {
                let update = Update::State(self.state.state_view());
                self.config.monitor.update(update);
            }

            self.hash = new_hash;
        }

        None
    }

    async fn signal_children(&mut self, sig: InternalSignal, k: Option<Arc<Notify>>) {
        let alive_hdls: Vec<_> = self
            .config
            .child_hdls
            .lock()
            .unwrap()
            .iter()
            .filter_map(|c| c.upgrade())
            .collect();

        let sig_ks = alive_hdls.iter().map(|c| c.signal(sig).into_future());

        join_all(sig_ks).await;

        if let Some(k) = k {
            k.notify_one()
        }
    }
}
