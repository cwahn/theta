use std::{
    any::Any,
    sync::{LazyLock, RwLock},
};

use anyhow::anyhow;
use rustc_hash::FxHashMap;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::{
    actor::{Actor, ActorId},
    actor_instance::K,
    actor_ref::ActorHdl,
    message::{Escalation, RawSignal},
};

// So the problem is how to attach monitor to actor instance.
// Type, ActorId seems what is required
// -> Is it only way to do it is keep the global map of Uuid?
// -> Rather don't want

// pub static MONITORS: RwLock<FxHashMap<ActorId, AnyMonitor>> = RwLock::new(FxHashMap::default());
// pub static MONITORS: LazyLock<RwLock<FxHashMap<ActorId, AnyMonitor>>> =
//     LazyLock::new(|| RwLock::new(FxHashMap::default()));

pub static ACTORS: LazyLock<RwLock<FxHashMap<ActorId, ActorHdl>>> =
    LazyLock::new(|| RwLock::new(FxHashMap::default()));

pub type AnyMonitor = Box<dyn Any + Send>;

// pub(crate) struct Monitor<A: Actor> {
//     pub(crate) is_listener: AtomicBool,
//     // todo Even though there is no listener, actor should keep it's last state report.
//     pub(crate) listener: Mutex<Vec<UnboundedSender<Report<A>>>>,
// }

pub(crate) struct Monitor<A: Actor> {
    pub(crate) observers: Vec<ReportTx<A>>,
}

pub type AnyReportTx = Box<dyn Any + Send>;

pub type ReportTx<A> = UnboundedSender<Report<A>>;
pub type ReportRx<A> = UnboundedReceiver<Report<A>>;

pub enum Report<A: Actor> {
    State(A::StateReport),
    Status(Status),
}

#[derive(Clone)]
pub enum Status {
    Processing,
    Paused,
    WaitingSignal,
    Resuming,

    Supervising(ActorId, Escalation),
    CleanupChildren,

    Panic(Escalation),
    Restarting,

    Dropping,
    Terminating,
}

pub fn observe_local<A: Actor>(
    actor_id: ActorId,
    tx: ReportTx<A>,
) -> anyhow::Result<Option<ReportTx<A>>> {
    let actors = ACTORS.read().unwrap();

    let Some(hdl) = actors.get(&actor_id) else {
        return Ok(Some(tx)); // Not found
    };

    if let Err(e) = hdl.raw_send(RawSignal::Observe(Box::new(tx.clone()) as AnyReportTx)) {
        return Err(anyhow!("Failed to send observe signal: {e}"));
    }

    Ok(None)
}

pub fn observe<A: Actor>(actor_id: ActorId, tx: ReportTx<A>) -> anyhow::Result<()> {
    match observe_local(actor_id, tx) {
        Ok(None) => Ok(()),
        // Ok(Some(tx)) => LocalPeer::get().observe::<A>(actor_id, tx),
        Ok(Some(tx)) => todo!(),
        Err(e) => Err(anyhow!("Failed to observe actor: {actor_id}, error: {e}")),
    }
}

// Implementations

impl<A> Monitor<A>
where
    A: Actor,
{
    pub fn add_observer(&mut self, tx: ReportTx<A>) {
        self.observers.push(tx);
    }

    pub fn report(&mut self, report: Report<A>) {
        if self.is_observer() {
            self.observers.retain(|tx| tx.send(report.clone()).is_ok());
        }
    }

    pub fn is_observer(&self) -> bool {
        !self.observers.is_empty()
    }
}

impl<A> Default for Monitor<A>
where
    A: Actor,
{
    fn default() -> Self {
        Monitor {
            observers: Vec::new(),
        }
    }
}

impl<A> Clone for Report<A>
where
    A: Actor,
{
    fn clone(&self) -> Self {
        match self {
            Report::State(state) => Report::State(state.clone()),
            Report::Status(status) => Report::Status(status.clone()),
        }
    }
}

impl From<&K> for Status {
    fn from(cont: &K) -> Self {
        match cont {
            K::Process => Status::Processing,

            K::Pause(_) => Status::Paused,
            K::WaitSignal => Status::WaitingSignal,
            K::Resume(_) => Status::Resuming,

            // ? Do I need ActorId here?
            K::Escalation(_, e) => Status::Supervising(ActorId::nil(), e.clone()),
            K::CleanupChildren => Status::CleanupChildren,

            K::Panic(e) => Status::Panic(e.clone()),
            K::Restart(_) => Status::Restarting,

            K::Drop => Status::Dropping,
            K::Terminate(_) => Status::Terminating,
        }
    }
}
