use std::{
    any::Any,
    sync::{LazyLock, RwLock},
};

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use theta_flume::{Receiver, Sender};

use crate::{
    actor::{Actor, ActorId},
    actor_instance::Cont,
    actor_ref::ActorHdl,
    context::{LookupError, ObserveError},
    message::Escalation,
};
#[cfg(feature = "remote")]
use {
    crate::{base::Ident, remote::base::RemoteError},
    url::Url,
};

pub static HDLS: LazyLock<RwLock<FxHashMap<ActorId, ActorHdl>>> =
    LazyLock::new(|| RwLock::new(FxHashMap::default()));

pub type AnyMonitor = Box<dyn Any + Send>;

pub type AnyReportTx = Box<dyn Any + Send>; // Type erased ReportTx<A>

pub type ReportTx<A> = Sender<Report<A>>;
pub type ReportRx<A> = Receiver<Report<A>>;

pub(crate) struct Monitor<A: Actor> {
    pub(crate) observers: Vec<ReportTx<A>>,
}

#[derive(Debug)]
#[cfg(feature = "remote")]
#[derive(Serialize, Deserialize)]
pub enum Report<A: Actor> {
    State(A::StateReport),
    Status(Status),
}

#[derive(Debug, Clone)]
#[cfg(feature = "remote")]
#[derive(Serialize, Deserialize)]
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

// ? What do I need to observe
// If only ident, then it should be possible to get hdl from actor.
// If local actor id, hard to the the information
// Fortunately, it is possible to get actor_id from ActorRef, get the id and then get handle
#[cfg(feature = "remote")]
pub async fn observe<A: Actor>(ident: impl AsRef<str>) -> anyhow::Result<()> {
    // match observe_local(actor_id, tx) {
    //     Ok(None) => Ok(()),
    //     // Ok(Some(tx)) => LocalPeer::get().observe::<A>(actor_id, tx),
    //     Ok(Some(_tx)) => todo!(),
    //     Err(e) => Err(anyhow!("Failed to observe actor: {actor_id}, error: {e}")),
    // }

    todo!()
}

#[cfg(feature = "remote")]
pub async fn observe_remote<A: Actor>(
    host_addr: &Url,
    ident: Ident,
    tx: ReportTx<A>,
) -> Result<(), RemoteError> {
    use crate::remote::peer::LocalPeer;

    let peer = LocalPeer::inst().get_or_connect(host_addr)?;

    peer.observe(ident, tx).await?;

    Ok(())
}

// Type should be already checked
pub fn observe_local<A: Actor>(actor_id: ActorId, tx: ReportTx<A>) -> Result<(), ObserveError> {
    let hdls = HDLS.read().unwrap();

    let hdl = hdls
        .get(&actor_id)
        .ok_or(ObserveError::LookupError(LookupError::NotFound))?;

    hdl.observe(Box::new(tx))
        .map_err(|_| ObserveError::SigSendError)?;

    Ok(())
}

// Implementations

impl<A: Actor> Monitor<A> {
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

impl<A: Actor> Default for Monitor<A> {
    fn default() -> Self {
        Monitor {
            observers: Vec::new(),
        }
    }
}

impl<A: Actor> Clone for Report<A> {
    fn clone(&self) -> Self {
        match self {
            Report::State(state) => Report::State(state.clone()),
            Report::Status(status) => Report::Status(status.clone()),
        }
    }
}

impl From<&Cont> for Status {
    fn from(cont: &Cont) -> Self {
        match cont {
            Cont::Process => Status::Processing,

            Cont::Pause(_) => Status::Paused,
            Cont::WaitSignal => Status::WaitingSignal,
            Cont::Resume(_) => Status::Resuming,

            // ? Do I need ActorId here?
            Cont::Supervise(_, e) => Status::Supervising(ActorId::nil(), e.clone()),
            Cont::CleanupChildren => Status::CleanupChildren,

            Cont::Panic(e) => Status::Panic(e.clone()),
            Cont::Restart(_) => Status::Restarting,

            Cont::Drop => Status::Dropping,
            Cont::Terminate(_) => Status::Terminating,
        }
    }
}
