extern crate self as theta;

pub mod actor;
pub mod actor_instance;
pub mod actor_ref;
pub mod base;
pub mod context;
pub mod errors;
pub mod message;
pub mod monitor;

#[cfg(feature = "persistence")]
// pub mod persistence;
#[cfg(feature = "remote")]
pub mod remote;

#[cfg(test)]
pub mod dev;

// Re-exports

pub mod prelude {
    pub use crate::actor::Actor;
    pub use crate::actor_ref::{ActorRef, WeakActorRef};
    pub use crate::context::Context;
    pub use crate::errors::{ExitCode, RequestError, SendError};
    pub use crate::message::{Escalation, Signal};

    pub use theta_macros::{ActorArgs, PersistentActor, actor};
}
