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
    actor_ref::{ActorHdl, WeakActorHdl, WeakActorRef},
    base::panic_msg,
    context::Context,
    message::{Continuation, DynMessage, Escalation, InternalSignal, RawSignal},
};

pub(crate) struct ActorConfig<A: Actor> {
    pub(crate) this: WeakActorRef<A>, // Self reference

    pub(crate) parent_hdl: ActorHdl,          // Parent handle
    pub(crate) this_hdl: ActorHdl,            // Self handle
    pub(crate) child_hdls: Vec<WeakActorHdl>, // Children of this actor

    pub(crate) sig_rx: UnboundedReceiver<RawSignal>, // Signal receiver
    pub(crate) msg_rx: UnboundedReceiver<(DynMessage<A>, Option<Continuation>)>, // Message receiver

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
    CleanupChildren,

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
        parent_hdl: ActorHdl,
        this_hdl: ActorHdl,
        sig_rx: UnboundedReceiver<RawSignal>,
        msg_rx: UnboundedReceiver<(DynMessage<A>, Option<Continuation>)>,
        args: A::Args,
    ) -> Self {
        let child_hdls = Vec::new();
        let mb_restart_k = None;

        Self {
            this,
            parent_hdl,
            this_hdl,
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

    async fn init_instance(self) -> Result<ActorInstance<A>, (Self, Escalation)> {
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

    fn ctx_args(&mut self) -> (Context<A>, &A::Args) {
        (
            Context {
                this: &self.this,
                child_hdls: &mut self.child_hdls,
                this_hdl: &self.this_hdl,
            },
            &self.args,
        )
    }

    fn ctx(&mut self) -> Context<A> {
        Context {
            this: &self.this,
            child_hdls: &mut self.child_hdls,
            this_hdl: &self.this_hdl,
        }
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

impl<A> ActorState<A>
where
    A: Actor,
{
    async fn init(mut config: ActorConfig<A>) -> Result<Self, (ActorConfig<A>, Escalation)> {
        let (ctx, args) = config.ctx_args();

        let init_res = AssertUnwindSafe(A::initialize(ctx, args))
            .catch_unwind()
            .await;

        if config.mb_restart_k.is_some() {
            config.mb_restart_k.take().unwrap().notify_one();
        }

        let state = match init_res {
            Ok(state) => state,
            Err(e) => {
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
                            None => return Cont::WaitSignal,
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
        // self.signal_children(SignalKind::Pause, k).await;

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
                let msg = panic_msg(e);
                #[cfg(feature = "tracing")]
                tracing::warn!("{} supervise panic: {}", type_name::<A>(), msg);
                return Cont::Panic(Some(Escalation::Supervise(msg)));
            }
        }

        Cont::Process
    }

    async fn cleanup_children(&mut self) -> Cont {
        self.config.child_hdls.retain(|c| c.0.strong_count() > 0);

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

    async fn restart(mut self, k: &mut Option<Arc<Notify>>) -> Lifecycle<A> {
        self.config.mb_restart_k = k.take();

        self.signal_children(InternalSignal::Restart, &mut None)
            .await;

        let res = AssertUnwindSafe(A::on_restart(&mut self.state))
            .catch_unwind()
            .await;

        if let Err(_e) = res {
            #[cfg(feature = "tracing")]
            tracing::warn!("{} on_restart panic: {}", type_name::<A>(), panic_msg(_e));
        }

        Lifecycle::Restarting(self.config)
    }

    async fn drop(&mut self) -> Lifecycle<A> {
        self.config.sig_rx.close();

        while let Some(sig) = self.config.sig_rx.recv().await {
            match sig {
                RawSignal::Pause(k)
                | RawSignal::Resume(k)
                | RawSignal::Restart(k)
                | RawSignal::Terminate(k) => {
                    k.map(|k| k.notify_one());
                }
                _s => {
                    #[cfg(feature = "tracing")]
                    tracing::error!(
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
            #[cfg(feature = "tracing")]
            tracing::warn!("{} on_exit panic: {}", type_name::<A>(), panic_msg(_e));
        }

        self.config
            .parent_hdl
            .raw_send(RawSignal::ChildDropped)
            .unwrap();

        Lifecycle::Exit
    }

    async fn terminate(&mut self, k: &mut Option<Arc<Notify>>) -> Lifecycle<A> {
        self.signal_children(InternalSignal::Terminate, k).await;

        let res = AssertUnwindSafe(A::on_exit(&mut self.state, ExitCode::Dropped))
            .catch_unwind()
            .await;

        if let Err(_e) = res {
            let msg = panic_msg(_e);
            #[cfg(feature = "tracing")]
            tracing::warn!("{} on_exit panic: {}", type_name::<A>(), msg);
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

    async fn process_msg(
        &mut self,
        (msg, k): (DynMessage<A>, Option<Continuation>),
    ) -> Option<Cont> {
        let ctx = self.config.ctx();

        let res = AssertUnwindSafe(self.state.process_msg(ctx, msg, k))
            .catch_unwind()
            .await;

        if let Err(e) = res {
            return Some(Cont::Panic(Some(Escalation::ProcessMsg(panic_msg(e)))));
        }

        None
    }

    async fn signal_children(&mut self, sig: InternalSignal, k: &mut Option<Arc<Notify>>) {
        let active_hdls = self
            .config
            .child_hdls
            .iter()
            .filter_map(|c| c.upgrade())
            .collect::<Vec<_>>();

        let pause_ks = active_hdls.iter().map(|c| c.signal(sig).into_future());

        join_all(pause_ks).await;

        k.take().map(|k| k.notify_one());
    }
}
