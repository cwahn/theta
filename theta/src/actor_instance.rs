use core::hash;
use std::{
    any::type_name,
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex, atomic},
};

use futures::{FutureExt, future::join_all};
use tokio::{
    select,
    sync::{
        Notify,
        mpsc::{UnboundedReceiver, error::TryRecvError},
    },
};

use tracing::{error, warn}; // For logging errors and warnings]

use crate::{
    actor::Actor,
    actor_ref::{ActorHdl, WeakActorHdl, WeakActorRef},
    base::panic_msg,
    // context::Context,
    error::ExitCode,
    message::{Continuation, InternalSignal, MsgRx, RawSignal, SigRx},
    monitor::{AnyReportTx, Monitor, Report, ReportTx, Status},
};

pub(crate) struct ActorState<A: Actor> {
    pub(crate) this: WeakActorRef<A>, // Self reference

    // pub(crate) parent_hdl: ActorHdl, // Parent handle
    pub(crate) this_hdl: ActorHdl, // Self handle
    // pub(crate) child_hdls: Arc<Mutex<Vec<WeakActorHdl>>>, // Children of this actor
    pub(crate) sig_rx: SigRx,
    pub(crate) msg_rx: MsgRx<A>,

    pub(crate) monitor: Monitor<A>, // Monitor for this actor

    // pub(crate) cfg: A::Args, // Arguments for actor initialization
    pub(crate) state: A,
    hash_code: u64, // Hash code for state report
                    //                          // pub(crate) mb_restart_k: Option<Arc<Notify>>, // Optional continuation for restart signal
}

pub(crate) struct ActorInstance<A: Actor> {
    k: Cont,
    state: ActorState<A>,
}

// pub(crate) struct __ActorState<A: Actor> {
//     state: A,
//     hash_code: u64,
//     config: ActorConfigImpl<A>,
// }

pub(crate) enum Lifecycle<A: Actor> {
    Running(ActorInstance<A>),
    Restarting(ActorState<A>),
    Exit,
}

// Continuation of an actor
pub(crate) enum Cont {
    Process,

    // Pause(Option<Arc<Notify>>),
    WaitSignal,
    // Resume(Option<Arc<Notify>>),

    // Escalation(ActorHdl, Escalation),
    // CleanupChildren,

    // Panic(Escalation),
    // Restart(Option<Arc<Notify>>),
    Drop,
    Terminate(Option<Arc<Notify>>),
}

// Implementations

impl<A> ActorState<A>
where
    A: Actor,
    // C: ActorConfig<Actor = A>,
{
    pub(crate) fn new(
        this: WeakActorRef<A>,
        // parent_hdl: ActorHdl,
        this_hdl: ActorHdl,
        sig_rx: UnboundedReceiver<RawSignal>,
        msg_rx: UnboundedReceiver<(A::Msg, Continuation)>,
        state: A,
    ) -> Self {
        // let child_hdls = Arc::new(Mutex::new(Vec::new()));
        // let mb_restart_k = None;
        let monitor = Monitor::default();
        let hash_code = state.hash_code();

        Self {
            this,
            // parent_hdl,
            this_hdl,
            // child_hdls,
            sig_rx,
            msg_rx,
            monitor,
            state,
            hash_code,
            // mb_restart_k,
        }
    }

    pub(crate) async fn exec(self) {
        let mut mb_config = Some(self);

        while let Some(config) = mb_config {
            mb_config = config.exec_impl().await;
        }
    }

    async fn exec_impl(self) -> Option<Self> {
        // let mb_inst = self.init_instance().await;

        // let inst = match mb_inst {
        //     Ok(inst) => inst,
        //     Err((config, e)) => {
        //         config
        //             .parent_hdl
        //             .escalate(config.this_hdl.clone(), e)
        //             .unwrap();

        //         return config.wait_signal().await;
        //     }
        // };

        let inst = ActorInstance {
            k: Cont::Process,
            state: self,
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

    // async fn init_instance(self) -> Result<ActorInstance<A>, (Self, Escalation)> {
    //     Ok(ActorInstance {
    //         k: Cont::Process,
    //         state: __ActorState::init(self).await?,
    //     })
    // }

    // async fn wait_signal(mut self) -> Option<Self> {
    //     let sig = self.sig_rx.recv().await.unwrap();

    //     match sig {
    //         // RawSignal::Restart(k) => {
    //         //     self.mb_restart_k = k;
    //         //     Some(self)
    //         // }
    //         _ => None,
    //     }
    // }

    // fn ctx_cfg(&mut self) -> (Context<A>, &C) {
    //     (
    //         Context {
    //             this: self.this.clone(),
    //             child_hdls: self.child_hdls.clone(),
    //             this_hdl: self.this_hdl.clone(),
    //         },
    //         &self.cfg,
    //     )
    // }

    // fn ctx(&mut self) -> Context<A> {
    //     Context {
    //         this: self.this.clone(),
    //         child_hdls: self.child_hdls.clone(),
    //         this_hdl: self.this_hdl.clone(),
    //     }
    // }
}

impl<A> ActorInstance<A>
where
    A: Actor,
    // C: ActorConfig<Actor = A>,
{
    async fn run(mut self) -> Lifecycle<A> {
        loop {
            self.state
                //
                .monitor
                .report(Report::Status((&self.k).into()));

            self.k = match self.k {
                Cont::Process => self.state.process().await,

                // Cont::Pause(k) => self.state.pause(k).await,
                Cont::WaitSignal => self.state.wait_signal().await,
                // Cont::Resume(k) => self.state.resume(k).await,

                // Cont::Escalation(c, e) => self.state.supervise(c, e).await,
                // Cont::CleanupChildren => self.state.cleanup_children().await,

                // Cont::Panic(e) => self.state.escalate(e).await,
                // Cont::Restart(k) => return self.state.restart(k).await,
                Cont::Drop => return self.state.drop().await,
                Cont::Terminate(k) => return self.state.terminate(k).await,
            };
        }
    }
}

impl<A> ActorState<A>
where
    A: Actor,
    // C: ActorConfig<Actor = A>,
{
    // async fn init(mut config: ActorState<A>) -> Result<Self, (ActorState<A>)> {
    //     let (ctx, cfg) = config.ctx_cfg();

    //     // let init_res = C::initialize(ctx, cfg).catch_unwind().await;
    //     let init_res = A::initialize(cfg).catch_unwind().await;

    //     // if config.mb_restart_k.is_some() {
    //     //     config.mb_restart_k.take().unwrap().notify_one();
    //     // }

    //     let state = match init_res {
    //         Ok(state) => state,
    //         Err(e) => {
    //             config.child_hdls.clear_poison();
    //             return Err((config, Escalation::Initialize(panic_msg(e))));
    //         }
    //     };

    //     let hash_code = state.hash_code();

    //     Ok(__ActorState {
    //         state,
    //         hash_code,
    //         config,
    //     })
    // }

    async fn process(&mut self) -> Cont {
        loop {
            loop {
                match self.sig_rx.try_recv() {
                    Ok(sig) => {
                        match self.process_sig(sig).await {
                            Some(k) => return k,
                            None => continue, // Continue processing signals
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => unreachable!("'this_hdl' is held by config"),
                }
            }

            match self.msg_rx.try_recv() {
                Ok(msg_k) => {
                    if let Some(k) = self.process_msg(msg_k).await {
                        return k;
                    }
                }
                Err(TryRecvError::Empty) => {
                    select! {
                        mb_sig = self.sig_rx.recv() => match self.process_sig(mb_sig.unwrap()).await {
                            Some(k) => return k,
                            None => continue, // Continue processing signals
                        },
                        mb_msg = self.msg_rx.recv() => match mb_msg {
                            Some(msg_k) => if let Some(k) = self.process_msg(msg_k).await {
                                return k;
                            },
                            None => {
                                if self.sig_rx.sender_strong_count() == 1 {
                                    return Cont::Drop;
                                }
                                return Cont::WaitSignal;
                            },
                        },
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    if self.sig_rx.sender_strong_count() == 1 {
                        return Cont::Drop;
                    }
                    return Cont::WaitSignal;
                }
            }
        }
    }

    async fn add_observer(&mut self, any_tx: AnyReportTx) {
        let Ok(tx) = any_tx.downcast::<ReportTx<A>>() else {
            return error!("{} received invalid observer", type_name::<A>(),);
        };

        self.monitor.add_observer(*tx);
        self.monitor
            .report(Report::State(self.state.state_report()));

        todo!("Add observer");
    }

    // async fn pause(&mut self, k: Option<Arc<Notify>>) -> Cont {
    //     self.signal_children(InternalSignal::Pause, k).await;

    //     Cont::WaitSignal
    // }

    async fn wait_signal(&mut self) -> Cont {
        loop {
            let sig = self.sig_rx.recv().await.unwrap();
            // Observe does not count in this context
            match self.process_sig(sig).await {
                Some(k) => return k,
                None => continue,
            }
        }
    }

    // async fn resume(&mut self, k: Option<Arc<Notify>>) -> Cont {
    //     self.signal_children(InternalSignal::Resume, k).await;

    //     Cont::Process
    // }

    // async fn supervise(&mut self, c: ActorHdl, e: Escalation) -> Cont {
    //     let res = AssertUnwindSafe(self.state.supervise(e))
    //         .catch_unwind()
    //         .await;

    //     match res {
    //         Ok((one, rest)) => {
    //             let active_hdls = self
    //                 .child_hdls
    //                 .lock()
    //                 .unwrap()
    //                 .iter()
    //                 .filter_map(|c| c.upgrade())
    //                 .collect::<Vec<_>>();

    //             let signals = active_hdls.iter().filter_map(|ac| {
    //                 if ac == &c {
    //                     Some(ac.signal(one.into()).into_future())
    //                 } else {
    //                     rest.map(|r| ac.signal(r.into()).into_future())
    //                 }
    //             });

    //             join_all(signals).await;
    //         }
    //         Err(e) => {
    //             self.child_hdls.clear_poison();

    //             let msg = panic_msg(e);

    //             warn!("{} supervise panic: {msg}", type_name::<A>());

    //             return Cont::Panic(Escalation::Supervise(msg));
    //         }
    //     }

    //     Cont::Process
    // }

    // async fn cleanup_children(&mut self) -> Cont {
    //     self.child_hdls
    //         .lock()
    //         .unwrap()
    //         .retain(|hdl| hdl.0.strong_count() > 0);

    //     Cont::Process
    // }

    // async fn escalate(&mut self, e: Escalation) -> Cont {
    //     self.signal_children(InternalSignal::Pause, None).await;

    //     let res = self.parent_hdl.escalate(self.this_hdl.clone(), e);

    //     if let Err(_) = res {
    //         return Cont::Terminate(None);
    //     }

    //     Cont::WaitSignal
    // }

    // async fn restart(mut self, k: Option<Arc<Notify>>) -> Lifecycle<A, C> {
    //     self.mb_restart_k = k;

    //     self.signal_children(InternalSignal::Restart, None).await;

    //     let res = AssertUnwindSafe(A::on_restart(&mut self.state))
    //         .catch_unwind()
    //         .await;

    //     if let Err(_e) = res {
    //         self.child_hdls.clear_poison();

    //         warn!("{} on_restart panic: {}", type_name::<A>(), panic_msg(_e));
    //     }

    //     Lifecycle::Restarting(self)
    // }

    async fn drop(&mut self) -> Lifecycle<A> {
        self.sig_rx.close();

        while let Some(sig) = self.sig_rx.recv().await {
            match sig {
                // RawSignal::Pause(k)
                // | RawSignal::Resume(k)
                // | RawSignal::Restart(k)
                RawSignal::Terminate(k) => {
                    if let Some(k) = k {
                        k.notify_one()
                    }
                }
                s => {
                    error!(
                        "{} received unexpected signal while dropping: {s:?}",
                        type_name::<A>()
                    );
                }
            }
        }

        // let res = AssertUnwindSafe(A::on_exit(&mut self.state, ExitCode::Dropped))
        //     .catch_unwind()
        //     .await;

        // if let Err(_e) = res {
        //     self.child_hdls.clear_poison();

        //     warn!("{} on_exit panic: {}", type_name::<A>(), panic_msg(_e));
        // }

        A::on_exit(&mut self.state, ExitCode::Dropped).await;

        // self.parent_hdl.raw_send(RawSignal::ChildDropped).unwrap();

        Lifecycle::Exit
    }

    async fn terminate(&mut self, k: Option<Arc<Notify>>) -> Lifecycle<A> {
        // self.signal_children(InternalSignal::Terminate, k).await;

        // let res = AssertUnwindSafe(A::on_exit(&mut self.state, ExitCode::Dropped))
        //     .catch_unwind()
        //     .await;

        // if let Err(_e) = res {
        //     self.child_hdls.clear_poison();

        //     let msg = panic_msg(_e);

        //     warn!("{} on_exit panic: {msg}", type_name::<A>());
        // }

        A::on_exit(&mut self.state, ExitCode::Dropped).await;

        Lifecycle::Exit
    }

    async fn process_sig(&mut self, sig: RawSignal) -> Option<Cont> {
        match sig {
            RawSignal::Observe(t) => {
                self.add_observer(t).await;
                return None;
            }

            // RawSignal::Escalation(c, e) => Some(Cont::Escalation(c, e)),
            // RawSignal::ChildDropped => Some(Cont::CleanupChildren),
            // RawSignal::Pause(k) => Some(Cont::Pause(k)),
            // RawSignal::Resume(k) => Some(Cont::Resume(k)),
            // RawSignal::Restart(k) => Some(Cont::Restart(k)),
            RawSignal::Terminate(k) => Some(Cont::Terminate(k)),
        }

        // This is the place where monitor can access
    }

    async fn process_msg(&mut self, (msg, k): (A::Msg, Continuation)) -> Option<Cont> {
        // let ctx = self.ctx();
        // let res = AssertUnwindSafe(self.state.process_msg(ctx, msg, k))
        //     .catch_unwind()
        //     .await;

        // if let Err(e) = res {
        //     // No state report on panic
        //     self.child_hdls.clear_poison();
        //     return Some(Cont::Panic(Escalation::ProcessMsg(panic_msg(e))));
        // }

        self.state.process_msg(msg, k).await;

        if self.monitor.is_observer() {
            let new_hash_code = self.state.hash_code();

            if new_hash_code != self.hash_code {
                self.monitor
                    .report(Report::State(self.state.state_report()));
            }

            self.hash_code = new_hash_code;
        }

        // self.report(Report::State(self.state.state_report()));

        None
    }

    // async fn signal_children(&mut self, sig: InternalSignal, k: Option<Arc<Notify>>) {
    //     let active_hdls = self
    //         .child_hdls
    //         .lock()
    //         .unwrap()
    //         .iter()
    //         .filter_map(|c| c.upgrade())
    //         .collect::<Vec<_>>();

    //     let pause_ks = active_hdls.iter().map(|c| c.signal(sig).into_future());

    //     join_all(pause_ks).await;

    //     if let Some(k) = k {
    //         k.notify_one()
    //     }
    // }
}
