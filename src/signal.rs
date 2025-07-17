use std::sync::Arc;

use serde::Serialize;
use tokio::sync::Notify;

use crate::{actor_ref::SupervisionRef, error::NetworkError};

/// Signals which can be sent from outside the actor system.
/// Pause and terminate signals can only be sent by internal logic of feteration.
pub enum Signal {
    Pause,
    Resume,
    Restart,
    /// Terminate an actor on completion of current message processing.
    /// - On going behavior will run to completion, and error or panic will be escalated.
    /// - Will not get escalated.
    Terminate,
    /// Imediately aborts an actor.
    /// - On going behavior will get aborted.
    /// - Will get escalated as `Escalation::Aborted`.
    Abort,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "remote", derive(serde::Serialize, serde::Deserialize))]
pub enum Escalation {
    // Should be serde supported if remote feature is enabled
    Error(String),
    // Error(Arc<dyn NetworkError>), // Should be serde supported if remote feature is enabled
    PanicOnStart(String),
    PanicOnMessage(String),
    PanicOnError(String),
    PanicOnEscalation(String),
}

#[derive(Debug)]
#[cfg_attr(feature = "remote", derive(serde::Serialize, serde::Deserialize))]
pub(crate) enum RawSignal {
    Escalation(SupervisionRef, Escalation),
    Pause(Option<Arc<Notify>>),
    Resume(Option<Arc<Notify>>),
    Restart(Option<Arc<Notify>>),
    Terminate(Option<Arc<Notify>>), // Notify when actor is terminated
    Abort(Option<Arc<Notify>>),
}

// Implementations

impl Into<RawSignal> for Signal {
    fn into(self) -> RawSignal {
        match self {
            Signal::Pause => RawSignal::Pause(None),
            Signal::Resume => RawSignal::Resume(None),
            Signal::Restart => RawSignal::Restart(None),
            Signal::Terminate => RawSignal::Terminate(None),
            Signal::Abort => RawSignal::Abort(None),
        }
    }
}
