use std::sync::Arc;

use futures::lock::Mutex;

use crate::{actor::Actor, actor_ref::SupervisionRef, signal::Escalation};

#[derive(Debug, Default)]
pub enum ActorStatus<A: Actor> {
    Starting,
    Running,
    Pausing,                                        // Awaiting for current message handling
    Paused,                                         // Paused for supervision
    Escalations(Vec<(SupervisionRef, Escalation)>), // Received escalations from children
    Error(Arc<Mutex<Option<A::Error>>>),            // On error should recovered or escallate
    EscalatedError(Arc<Mutex<Option<A::Error>>>),   // Escalated error and awiting for directive
    Restarting,                                     // Restarting actor
    Dropping,                                       // Both channel closed, channel is empty
    Dropped,                                        //
    Terminating,                                    // Awaiting for termination of children
    Terminated,                                     // Fully terminated
    PanicOnStart(String),
    PanicOnMessage(String),
    PanicOnEscalation(String),
    PanicOnError(String),
    Aborted,
    #[default]
    Vacant, // No actor instance is running
}

pub(crate) enum MessageLoopStatus<A: Actor> {
    // Should wait for message and process it
    Running,
    Pausing,
    // Escalation to handle
    Escalation(SupervisionRef, Escalation),
    // Should envoke on_error which will recover or escalate
    Error(Arc<Mutex<Option<A::Error>>>),
    // Awaiting for directive from parent
    EscalatedError(Arc<Mutex<Option<A::Error>>>), // Escalated error and awaiting for directive
    // Should turn to Paused and wait for resume signal
    // Dropped,
    // Restarting actor
    Restarting,
    Terminating,
}

// Implementations

impl<A> ActorStatus<A>
where
    A: Actor,
{
    pub(crate) fn message_loop_status(&self) -> MessageLoopStatus<A> {
        match self {
            ActorStatus::Running => MessageLoopStatus::Running,
            ActorStatus::Pausing => MessageLoopStatus::Pausing,
            ActorStatus::Escalations(s, e) => MessageLoopStatus::Escalation(s.clone(), e.clone()),
            ActorStatus::Error(err) => MessageLoopStatus::Error(err.clone()),
            ActorStatus::EscalatedError(err) => MessageLoopStatus::EscalatedError(err.clone()),
            ActorStatus::Restarting => MessageLoopStatus::Restarting,
            ActorStatus::Terminating => MessageLoopStatus::Terminating,
            _ => unreachable!(),
        }
    }
}

impl<A> Clone for ActorStatus<A>
where
    A: Actor,
{
    fn clone(&self) -> Self {
        match self {
            ActorStatus::Starting => ActorStatus::Starting,
            ActorStatus::Running => ActorStatus::Running,
            ActorStatus::Pausing => ActorStatus::Pausing,
            ActorStatus::Paused => ActorStatus::Paused,
            ActorStatus::Escalations(s, e) => ActorStatus::Escalations(s.clone(), e.clone()),
            ActorStatus::Error(err) => ActorStatus::Error(err.clone()),
            ActorStatus::EscalatedError(err) => ActorStatus::EscalatedError(err.clone()),
            ActorStatus::Restarting => ActorStatus::Restarting,
            ActorStatus::Dropping => ActorStatus::Dropping,
            ActorStatus::Dropped => ActorStatus::Dropped,
            ActorStatus::Terminating => ActorStatus::Terminating,
            ActorStatus::Terminated => ActorStatus::Terminated,
            ActorStatus::PanicOnStart(msg) => ActorStatus::PanicOnStart(msg.clone()),
            ActorStatus::PanicOnMessage(msg) => ActorStatus::PanicOnMessage(msg.clone()),
            ActorStatus::PanicOnEscalation(msg) => ActorStatus::PanicOnEscalation(msg.clone()),
            ActorStatus::PanicOnError(msg) => ActorStatus::PanicOnError(msg.clone()),
            ActorStatus::Aborted => ActorStatus::Aborted,
            ActorStatus::Vacant => ActorStatus::Vacant,
        }
    }
}
