use std::{
    any::type_name,
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex},
};

use futures::{FutureExt, future::join_all};
use tokio::{
    select,
    sync::{
        Notify,
        mpsc::{UnboundedReceiver, error::TryRecvError},
    },
};
#[cfg(feature = "tracing")]
use tracing::{error, warn}; // For logging errors and warnings]

use crate::{
    actor::{Actor, ActorConfig},
    actor_ref::{ActorHdl, WeakActorHdl, WeakActorRef},
    base::panic_msg,
    context::Context,
    error::ExitCode,
    message::{Continuation, DynMessage, Escalation, InternalSignal, RawSignal},
};

pub(crate) struct ActorConfigImpl<A: Actor, C: ActorConfig<Actor = A>> {
    pub(crate) this: WeakActorRef<A>, // Self reference

    pub(crate) parent_hdl: ActorHdl, // Parent handle
    pub(crate) this_hdl: ActorHdl,   // Self handle
    pub(crate) child_hdls: Arc<Mutex<Vec<WeakActorHdl>>>, // Children of this actor

    pub(crate) sig_rx: UnboundedReceiver<RawSignal>, // Signal receiver
    pub(crate) msg_rx: UnboundedReceiver<(DynMessage<A>, Continuation)>, // Message receiver

    pub(crate) cfg: C, // Arguments for actor initialization

    pub(crate) mb_restart_k: Option<Arc<Notify>>, // Optional continuation for restart signal
}

struct ActorInstance<A: Actor, C: ActorConfig<Actor = A>> {
    k: Cont,
    state: ActorState<A, C>,
}

struct ActorState<A: Actor, C: ActorConfig<Actor = A>> {
    state: A,
    config: ActorConfigImpl<A, C>,
}

enum Lifecycle<A: Actor, C: ActorConfig<Actor = A>> {
    Running(ActorInstance<A, C>),
    Restarting(ActorConfigImpl<A, C>),
    Exit,
}

// Continuation of an actor
enum Cont {
    Process,

    Pause(Option<Arc<Notify>>),
    WaitSignal,
    Resume(Option<Arc<Notify>>),

    Escalation(Option<ActorHdl>, Option<Escalation>),
    CleanupChildren,

    Panic(Option<Escalation>),
    Restart(Option<Arc<Notify>>),

    Drop,
    Terminate(Option<Arc<Notify>>),
}

// Implementations

impl<A, C> ActorConfigImpl<A, C>
where
    A: Actor,
    C: ActorConfig<Actor = A>,
{
    pub(crate) fn new(
        this: WeakActorRef<A>,
        parent_hdl: ActorHdl,
        this_hdl: ActorHdl,
        sig_rx: UnboundedReceiver<RawSignal>,
        msg_rx: UnboundedReceiver<(DynMessage<A>, Continuation)>,
        cfg: C,
    ) -> Self {
        let child_hdls = Arc::new(Mutex::new(Vec::new()));
        let mb_restart_k = None;

        Self {
            this,
            parent_hdl,
            this_hdl,
            child_hdls,
            sig_rx,
            msg_rx,
            cfg,
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

        let inst = match mb_inst {
            Ok(inst) => inst,
            Err((config, e)) => {
                config
                    .parent_hdl
                    .escalate(config.this_hdl.clone(), e)
                    .unwrap();

                return config.wait_signal().await;
            }
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

    async fn init_instance(self) -> Result<ActorInstance<A, C>, (Self, Escalation)> {
        Ok(ActorInstance {
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

    fn ctx_cfg(&mut self) -> (Context<A>, &C) {
        (
            Context {
                this: self.this.clone(),
                child_hdls: self.child_hdls.clone(),
                this_hdl: self.this_hdl.clone(),
            },
            &self.cfg,
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

impl<A, C> ActorInstance<A, C>
where
    A: Actor,
    C: ActorConfig<Actor = A>,
{
    async fn run(mut self) -> Lifecycle<A, C> {
        loop {
            self.k = match &mut self.k {
                Cont::Process => self.state.process().await,
                Cont::Pause(k) => self.state.pause(k).await,
                Cont::WaitSignal => self.state.wait_signal().await,
                Cont::Resume(k) => self.state.resume(k).await,

                Cont::Escalation(s @ Some(_), e @ Some(_)) => self.state.supervise(s, e).await,
                Cont::Escalation(_, _) => unreachable!(),

                Cont::CleanupChildren => self.state.cleanup_children().await,

                Cont::Panic(e @ Some(_)) => self.state.escalate(e).await,
                Cont::Panic(None) => unreachable!(),

                Cont::Restart(k) => return self.state.restart(k).await,
                Cont::Drop => return self.state.drop().await,
                Cont::Terminate(k) => return self.state.terminate(k).await,
            };
        }
    }
}

impl<A, C> ActorState<A, C>
where
    A: Actor,
    C: ActorConfig<Actor = A>,
{
    async fn init(
        mut config: ActorConfigImpl<A, C>,
    ) -> Result<Self, (ActorConfigImpl<A, C>, Escalation)> {
        let (ctx, cfg) = config.ctx_cfg();

        let init_res = C::initialize(ctx, cfg).catch_unwind().await;

        if config.mb_restart_k.is_some() {
            config.mb_restart_k.take().unwrap().notify_one();
        }

        let state = match init_res {
            Ok(state) => state,
            Err(e) => {
                config.child_hdls.clear_poison();
                return Err((config, Escalation::Initialize(panic_msg(e))));
            }
        };

        Ok(ActorState { config, state })
    }

    async fn process(&mut self) -> Cont {
        loop {
            loop {
                match self.config.sig_rx.try_recv() {
                    Ok(sig) => return self.process_sig(sig).await,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => unreachable!("'this_hdl' is held by config"),
                }
            }

            match self.config.msg_rx.try_recv() {
                Ok(msg_k) => {
                    if let Some(k) = self.process_msg(msg_k).await {
                        return k;
                    }
                }
                Err(TryRecvError::Empty) => {
                    select! {
                        mb_sig = self.config.sig_rx.recv() => return self.process_sig(mb_sig.unwrap()).await,
                        mb_msg = self.config.msg_rx.recv() => match mb_msg {
                            Some(msg_k) => if let Some(k) = self.process_msg(msg_k).await {
                                return k;
                            },
                            None => {
                                if self.config.sig_rx.sender_strong_count() == 1 {
                                    return Cont::Drop;
                                }
                                return Cont::WaitSignal;
                            },
                        },
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    if self.config.sig_rx.sender_strong_count() == 1 {
                        return Cont::Drop;
                    }
                    return Cont::WaitSignal;
                }
            }
        }
    }

    async fn pause(&mut self, k: &mut Option<Arc<Notify>>) -> Cont {
        self.signal_children(InternalSignal::Pause, k).await;

        Cont::WaitSignal
    }

    async fn wait_signal(&mut self) -> Cont {
        let sig = self.config.sig_rx.recv().await.unwrap();

        self.process_sig(sig).await
    }

    async fn resume(&mut self, k: &mut Option<Arc<Notify>>) -> Cont {
        self.signal_children(InternalSignal::Resume, k).await;

        Cont::Process
    }

    async fn supervise(&mut self, c: &mut Option<ActorHdl>, e: &mut Option<Escalation>) -> Cont {
        let c = c.take().unwrap();
        let e = e.take().unwrap();

        let res = AssertUnwindSafe(self.state.supervise(e))
            .catch_unwind()
            .await;

        match res {
            Ok((one, rest)) => {
                let active_hdls = self
                    .config
                    .child_hdls
                    .lock()
                    .unwrap()
                    .iter()
                    .filter_map(|c| c.upgrade())
                    .collect::<Vec<_>>();

                let signals = active_hdls.iter().filter_map(|ac| {
                    if ac == &c {
                        Some(ac.signal(one.into()).into_future())
                    } else {
                        rest.map(|r| ac.signal(r.into()).into_future())
                    }
                });

                join_all(signals).await;
            }
            Err(e) => {
                self.config.child_hdls.clear_poison();

                let msg = panic_msg(e);
                #[cfg(feature = "tracing")]
                warn!("{} supervise panic: {}", type_name::<A>(), msg);
                return Cont::Panic(Some(Escalation::Supervise(msg)));
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

    async fn escalate(&mut self, e: &mut Option<Escalation>) -> Cont {
        let e = e.take().unwrap();

        self.signal_children(InternalSignal::Pause, &mut None).await;

        let res = self
            .config
            .parent_hdl
            .escalate(self.config.this_hdl.clone(), e);

        if let Err(_) = res {
            return Cont::Terminate(None);
        }

        Cont::WaitSignal
    }

    async fn restart(mut self, k: &mut Option<Arc<Notify>>) -> Lifecycle<A, C> {
        self.config.mb_restart_k = k.take();

        self.signal_children(InternalSignal::Restart, &mut None)
            .await;

        let res = AssertUnwindSafe(A::on_restart(&mut self.state))
            .catch_unwind()
            .await;

        if let Err(_e) = res {
            self.config.child_hdls.clear_poison();

            #[cfg(feature = "tracing")]
            warn!("{} on_restart panic: {}", type_name::<A>(), panic_msg(_e));
        }

        Lifecycle::Restarting(self.config)
    }

    async fn drop(&mut self) -> Lifecycle<A, C> {
        self.config.sig_rx.close();

        while let Some(sig) = self.config.sig_rx.recv().await {
            match sig {
                RawSignal::Pause(k)
                | RawSignal::Resume(k)
                | RawSignal::Restart(k)
                | RawSignal::Terminate(k) => {
                    if let Some(k) = k {
                        k.notify_one()
                    }
                }
                _s => {
                    #[cfg(feature = "tracing")]
                    error!(
                        "{} received unexpected signal while dropping: {:?}",
                        type_name::<A>(),
                        _s
                    );
                }
            }
        }

        let res = AssertUnwindSafe(A::on_exit(&mut self.state, ExitCode::Dropped))
            .catch_unwind()
            .await;

        if let Err(_e) = res {
            self.config.child_hdls.clear_poison();

            #[cfg(feature = "tracing")]
            warn!("{} on_exit panic: {}", type_name::<A>(), panic_msg(_e));
        }

        self.config
            .parent_hdl
            .raw_send(RawSignal::ChildDropped)
            .unwrap();

        Lifecycle::Exit
    }

    async fn terminate(&mut self, k: &mut Option<Arc<Notify>>) -> Lifecycle<A, C> {
        self.signal_children(InternalSignal::Terminate, k).await;

        let res = AssertUnwindSafe(A::on_exit(&mut self.state, ExitCode::Dropped))
            .catch_unwind()
            .await;

        if let Err(_e) = res {
            self.config.child_hdls.clear_poison();

            let msg = panic_msg(_e);
            #[cfg(feature = "tracing")]
            warn!("{} on_exit panic: {}", type_name::<A>(), msg);
        }

        Lifecycle::Exit
    }

    async fn process_sig(&mut self, sig: RawSignal) -> Cont {
        match sig {
            RawSignal::Escalation(s, e) => Cont::Escalation(Some(s), Some(e)),
            RawSignal::ChildDropped => Cont::CleanupChildren,

            RawSignal::Pause(k) => Cont::Pause(k),
            RawSignal::Resume(k) => Cont::Resume(k),
            RawSignal::Restart(k) => Cont::Restart(k),
            RawSignal::Terminate(k) => Cont::Terminate(k),
        }
    }

    async fn process_msg(&mut self, (msg, k): (DynMessage<A>, Continuation)) -> Option<Cont> {
        let ctx = self.config.ctx();

        let res = AssertUnwindSafe(self.state.process_msg(ctx, msg, k))
            .catch_unwind()
            .await;

        if let Err(e) = res {
            self.config.child_hdls.clear_poison();

            return Some(Cont::Panic(Some(Escalation::ProcessMsg(panic_msg(e)))));
        }

        None
    }

    async fn signal_children(&mut self, sig: InternalSignal, k: &mut Option<Arc<Notify>>) {
        let active_hdls = self
            .config
            .child_hdls
            .lock()
            .unwrap()
            .iter()
            .filter_map(|c| c.upgrade())
            .collect::<Vec<_>>();

        let pause_ks = active_hdls.iter().map(|c| c.signal(sig).into_future());

        join_all(pause_ks).await;

        if let Some(k) = k.take() {
            k.notify_one()
        }
    }
}
