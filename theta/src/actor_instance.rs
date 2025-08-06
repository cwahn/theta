use std::{any::type_name, sync::Arc};

use tokio::{
    select,
    sync::{
        Notify,
        mpsc::{UnboundedReceiver, error::TryRecvError},
    },
};

use crate::{
    actor::Actor,
    actor_ref::{ActorHdl, WeakActorRef},
    error::ExitCode,
    message::{Continuation, MsgRx, RawSignal, SigRx},
    monitor::{AnyReportTx, Monitor, Report, ReportTx},
};
use tracing::error; // For logging errors and warnings]

pub(crate) struct ActorState<A: Actor> {
    pub(crate) this: WeakActorRef<A>, // Self reference

    pub(crate) this_hdl: ActorHdl, // Self handle

    pub(crate) sig_rx: SigRx,
    pub(crate) msg_rx: MsgRx<A>,

    pub(crate) monitor: Monitor<A>, // Monitor for this actor

    pub(crate) state: A,
    hash_code: u64, // Hash code for state report
                    //                          // pub(crate) mb_restart_k: Option<Arc<Notify>>, // Optional continuation for restart signal
}

pub(crate) struct ActorInstance<A: Actor> {
    k: Cont,
    state: ActorState<A>,
}

pub(crate) enum Lifecycle<A: Actor> {
    Running(ActorInstance<A>),
    Restarting(ActorState<A>),
    Exit,
}

// Continuation of an actor
pub(crate) enum Cont {
    Process,

    WaitSignal,

    Drop,
    Terminate(Option<Arc<Notify>>),
}

// Implementations

impl<A> ActorState<A>
where
    A: Actor,
{
    pub(crate) fn new(
        this: WeakActorRef<A>,
        this_hdl: ActorHdl,
        sig_rx: UnboundedReceiver<RawSignal>,
        msg_rx: UnboundedReceiver<(A::Msg, Continuation)>,
        state: A,
    ) -> Self {
        let monitor = Monitor::default();
        let hash_code = state.hash_code();

        Self {
            this,
            this_hdl,
            sig_rx,
            msg_rx,
            monitor,
            state,
            hash_code,
        }
    }

    pub(crate) async fn exec(self) {
        let mut mb_config = Some(self);

        while let Some(config) = mb_config {
            mb_config = config.exec_impl().await;
        }
    }

    async fn exec_impl(self) -> Option<Self> {
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

                Cont::WaitSignal => self.state.wait_signal().await,

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

    async fn drop(&mut self) -> Lifecycle<A> {
        self.sig_rx.close();

        while let Some(sig) = self.sig_rx.recv().await {
            match sig {
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

        A::on_exit(&mut self.state, ExitCode::Dropped).await;

        Lifecycle::Exit
    }

    async fn terminate(&mut self, k: Option<Arc<Notify>>) -> Lifecycle<A> {
        A::on_exit(&mut self.state, ExitCode::Dropped).await;

        Lifecycle::Exit
    }

    async fn process_sig(&mut self, sig: RawSignal) -> Option<Cont> {
        match sig {
            RawSignal::Observe(t) => {
                self.add_observer(t).await;
                return None;
            }

            RawSignal::Terminate(k) => Some(Cont::Terminate(k)),
        }

        // This is the place where monitor can access
    }

    async fn process_msg(&mut self, (msg, k): (A::Msg, Continuation)) -> Option<Cont> {
        self.state.process_msg(msg, k).await;

        if self.monitor.is_observer() {
            let new_hash_code = self.state.hash_code();

            if new_hash_code != self.hash_code {
                self.monitor
                    .report(Report::State(self.state.state_report()));
            }

            self.hash_code = new_hash_code;
        }

        None
    }
}
