use std::{
    any::type_name,
    fmt::Display,
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex},
};

use futures::{FutureExt, channel::oneshot, future::Either, future::select};
use pin_utils::pin_mut;
use tracing::error;

#[cfg(feature = "monitor")]
use crate::monitor::{AnyUpdateTx, Monitor, Update, UpdateTx};
use crate::{
    actor::{Actor, ActorArgs, ExitCode},
    actor_ref::{ActorHdl, WeakActorRef},
    base::panic_msg,
    context::Context,
    message::{Continuation, Escalation, InternalSignal, MsgRx, RawSignal, SigAck, SigRx},
};

const DROP_HDL_COUNT: usize = if cfg!(feature = "monitor") { 2 } else { 1 };

/// Configuration and runtime resources for an actor instance.
pub struct ActorConfig<A: Actor, Args: ActorArgs<Actor = A>> {
    pub(crate) ctx: Context<A>,
    pub(crate) parent_hdl: ActorHdl,
    pub(crate) sig_rx: SigRx,
    pub(crate) msg_rx: MsgRx<A>,
    #[cfg(feature = "monitor")]
    pub(crate) monitor: Monitor<A>,
    pub(crate) args: Args,
    pub(crate) mb_restart_k: Option<SigAck>,
}

/// Actor state container with configuration and hash.
pub struct ActorState<A: Actor, Args: ActorArgs<Actor = A>> {
    state: A,
    #[cfg(feature = "monitor")]
    hash: u64,
    config: ActorConfig<A, Args>,
}

/// Execution continuation states for actor processing.
pub enum Cont {
    Process,
    Pause(Option<SigAck>),
    WaitSignal,
    Resume(Option<SigAck>),
    Supervise(ActorHdl, Escalation),
    CleanupChildren,
    Panic(Escalation),
    Restart(Option<SigAck>),
    Drop,
    Terminate(Option<SigAck>),
}

/// Active actor instance with its execution continuation.
pub struct ActorInst<A: Actor, Args: ActorArgs<Actor = A>> {
    k: Cont,
    state: ActorState<A, Args>,
}

/// Actor lifecycle states for runtime management.
pub enum Lifecycle<A: Actor, Args: ActorArgs<Actor = A>> {
    Running(ActorInst<A, Args>),
    Restarting(ActorConfig<A, Args>),
    Exit,
}

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
        let ctx = Context {
            this,
            this_hdl,
            child_hdls,
        };
        let mb_restart_k = None;
        #[cfg(feature = "monitor")]
        let monitor = Monitor::default();

        Self {
            ctx,
            parent_hdl,
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
                    .escalate(config.ctx.this_hdl.clone(), e)
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
            RawSignal::Terminate(Some(k))
            | RawSignal::Pause(Some(k))
            | RawSignal::Resume(Some(k)) => {
                let _ = k.send(());

                None
            }
            _ => None,
        }
    }

    fn ctx_cfg(&self) -> (Context<A>, &Args) {
        (self.ctx.clone(), &self.args)
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
            if self.state.config.monitor.is_monitor() {
                self.state
                    .config
                    .monitor
                    .update(&Update::Status((&self.k).into()));
            }

            self.k = match self.k {
                Cont::Process => self.state.process().await,
                Cont::Pause(k) => self.state.pause(k).await,
                Cont::WaitSignal => self.state.wait_signal().await,
                Cont::Resume(k) => self.state.resume(k).await,
                Cont::Supervise(c, e) => self.state.supervise(c, e).await,
                Cont::CleanupChildren => self.state.cleanup_children(),
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
            let _ = k.send(());
        }

        let state = match init_res {
            Err(e) => {
                config.ctx.child_hdls.clear_poison();

                return Err((config, Escalation::Initialize(panic_msg(e))));
            }
            Ok(state) => state,
        };

        #[cfg(feature = "monitor")]
        let hash_code = state.hash_code();

        Ok(Self {
            state,
            #[cfg(feature = "monitor")]
            hash: hash_code,
            config,
        })
    }

    async fn process(&mut self) -> Cont {
        enum Recv<S, M> {
            Sig(S),
            Msg(Option<M>),
        }

        loop {
            let recv = match self.config.sig_rx.try_recv() {
                Err(_) => {
                    let sig_fut = self.config.sig_rx.recv();
                    let msg_fut = self.config.msg_rx.recv();

                    pin_mut!(sig_fut);
                    pin_mut!(msg_fut);

                    match select(sig_fut, msg_fut).await {
                        Either::Left((mb_sig, _)) => Recv::Sig(mb_sig.unwrap()),
                        Either::Right((mb_msg_k, _)) => Recv::Msg(mb_msg_k),
                    }
                }
                Ok(sig) => Recv::Sig(sig),
            };

            match recv {
                Recv::Sig(sig) => match self.process_sig(sig) {
                    None => {}
                    Some(k) => return k,
                },
                Recv::Msg(None) => {
                    if self.config.sig_rx.sender_count() <= DROP_HDL_COUNT {
                        return Cont::Drop;
                    }

                    return Cont::WaitSignal;
                }
                Recv::Msg(Some(msg_k)) => {
                    if let Some(k) = self.process_msg(msg_k).await {
                        return k;
                    }
                }
            }
        }
    }

    #[cfg(feature = "monitor")]
    fn add_monitor(&mut self, any_tx: AnyUpdateTx) {
        let Ok(tx) = any_tx.downcast::<UpdateTx<A>>() else {
            return error!(actor = % self, "received invalid monitor");
        };

        if let Err(err) = tx.send(Update::State(self.state.state_view())) {
            return error!(actor = % self, % err, "failed to send initial state update");
        }

        self.config.monitor.add_monitor(*tx);
    }

    async fn pause(&mut self, k: Option<SigAck>) -> Cont {
        self.signal_children(InternalSignal::Pause, k).await;

        Cont::WaitSignal
    }

    async fn wait_signal(&mut self) -> Cont {
        loop {
            let sig = self.config.sig_rx.recv().await.unwrap();

            match self.process_sig(sig) {
                None => {}
                Some(k) => return k,
            }
        }
    }

    async fn resume(&mut self, k: Option<SigAck>) -> Cont {
        self.signal_children(InternalSignal::Resume, k).await;

        Cont::Process
    }

    async fn supervise(&mut self, child_hdl: ActorHdl, escalation: Escalation) -> Cont {
        let res = AssertUnwindSafe(self.state.supervise(escalation))
            .catch_unwind()
            .await;

        match res {
            Err(e) => {
                self.config.ctx.child_hdls.clear_poison();

                return Cont::Panic(Escalation::Supervise(panic_msg(e)));
            }
            Ok((one, rest)) => {
                let acks: Vec<_> = {
                    let mut hdls = self.config.ctx.child_hdls.lock().unwrap();
                    let mut acks = Vec::with_capacity(hdls.len());

                    hdls.retain(|weak| match weak.upgrade() {
                        Some(hdl) => {
                            let sig: InternalSignal = if hdl == child_hdl {
                                one.into()
                            } else if let Some(r) = rest {
                                r.into()
                            } else {
                                return true;
                            };

                            let (tx, rx) = oneshot::channel();

                            if hdl.raw_send(sig.into_raw(Some(tx))).is_ok() {
                                acks.push(rx);
                            }

                            true
                        }
                        None => false,
                    });

                    acks
                };

                for ack in acks {
                    let _ = ack.await;
                }
            }
        }

        Cont::Process
    }

    fn cleanup_children(&self) -> Cont {
        self.config
            .ctx
            .child_hdls
            .lock()
            .unwrap()
            .retain(|hdl| match hdl.upgrade() {
                None => false,
                Some(hdl) => hdl.0.sender_count() > 0,
            });

        Cont::Process
    }

    async fn escalate(&mut self, e: Escalation) -> Cont {
        self.signal_children(InternalSignal::Pause, None).await;

        let res = self
            .config
            .parent_hdl
            .escalate(self.config.ctx.this_hdl.clone(), e);

        if res.is_err() {
            return Cont::Terminate(None);
        }

        Cont::WaitSignal
    }

    async fn restart(mut self, k: Option<SigAck>) -> Lifecycle<A, Args> {
        self.config.mb_restart_k = k;
        self.signal_children(InternalSignal::Terminate, None).await;

        let res = AssertUnwindSafe(A::on_restart(&mut self.state))
            .catch_unwind()
            .await;

        if let Err(e) = res {
            self.config.ctx.child_hdls.clear_poison();

            error!(actor = % self, err = panic_msg(e), "paniced on restart");
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
                        let _ = k.send(());
                    }
                }
                s => error!(actor = % self, sig = ? s, "received unexpected signal while dropping"),
            }
        }

        let res = AssertUnwindSafe(A::on_exit(&mut self.state, ExitCode::Dropped))
            .catch_unwind()
            .await;

        if let Err(e) = res {
            self.config.ctx.child_hdls.clear_poison();

            error!(actor = % self, err = panic_msg(e), "paniced on exit");
        }

        self.config
            .parent_hdl
            .raw_send(RawSignal::ChildDropped)
            .expect("parent lives longer than child");

        Lifecycle::Exit
    }

    async fn terminate(&mut self, k: Option<SigAck>) -> Lifecycle<A, Args> {
        self.signal_children(InternalSignal::Terminate, None).await;

        let res = AssertUnwindSafe(A::on_exit(&mut self.state, ExitCode::Terminated))
            .catch_unwind()
            .await;

        if let Err(e) = res {
            self.config.ctx.child_hdls.clear_poison();

            error!(actor = % self, err = panic_msg(e), "paniced on exit");
        }

        if let Some(k) = k {
            let _ = k.send(());
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
    }

    async fn process_msg(&mut self, (msg, k): (A::Msg, Continuation)) -> Option<Cont> {
        let res = AssertUnwindSafe(self.state.process_msg(&self.config.ctx, msg, k))
            .catch_unwind()
            .await;

        if let Err(e) = res {
            self.config.ctx.child_hdls.clear_poison();

            return Some(Cont::Panic(Escalation::ProcessMsg(panic_msg(e))));
        }

        #[cfg(feature = "monitor")]
        if self.config.monitor.is_monitor() {
            let new_hash = self.state.hash_code();

            if new_hash != self.hash {
                let update = Update::State(self.state.state_view());

                self.config.monitor.update(&update);
            }

            self.hash = new_hash;
        }

        None
    }

    #[allow(clippy::needless_pass_by_ref_mut)]
    async fn signal_children(&mut self, sig: InternalSignal, k: Option<SigAck>) {
        let acks: Vec<_> = {
            let mut hdls = self.config.ctx.child_hdls.lock().unwrap();
            let mut acks = Vec::with_capacity(hdls.len());

            hdls.retain(|weak| match weak.upgrade() {
                Some(hdl) => {
                    let (tx, rx) = oneshot::channel();

                    if hdl.raw_send(sig.into_raw(Some(tx))).is_ok() {
                        acks.push(rx);
                    }

                    true
                }
                None => false,
            });

            acks
        };

        for ack in acks {
            let _ = ack.await;
        }

        if let Some(k) = k {
            let _ = k.send(());
        }
    }
}

impl<A, Arg> Display for ActorState<A, Arg>
where
    A: Actor,
    Arg: ActorArgs<Actor = A>,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", type_name::<A>(), self.config.ctx.this_hdl.id())
    }
}
