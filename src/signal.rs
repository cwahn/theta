use std::sync::Arc;

use tokio::sync::Notify;

use crate::actor_ref::SupervisionRef;

/// Signals which can be sent from outside the actor system.
/// Pause and terminate signals can only be sent by internal logic of feteration.
pub enum Signal {
    Pause,
    Resume,
    Restart,
    Terminate,
}

#[derive(Debug, Clone)]
pub enum Escalation {
    Error(String),
    PanicOnStart(String),
    PanicOnMessage(String),
    PanicOnEscalation(String),
}

#[derive(Debug)]
pub(crate) enum RawSignal {
    Escalation(SupervisionRef, Escalation),
    Pause(Option<Arc<Notify>>),
    Resume(Option<Arc<Notify>>),
    Restart(Option<Arc<Notify>>),
    Terminate(Option<Arc<Notify>>),
}

// Implementations

impl Into<RawSignal> for Signal {
    fn into(self) -> RawSignal {
        match self {
            Signal::Pause => RawSignal::Pause(None),
            Signal::Resume => RawSignal::Resume(None),
            Signal::Restart => RawSignal::Restart(None),
            Signal::Terminate => RawSignal::Terminate(None),
        }
    }
}
