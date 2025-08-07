extern crate self as theta;

pub mod actor;
pub mod actor_instance;
pub mod actor_ref;
pub mod base;
pub mod context;
pub mod error;
pub mod global_context;
pub mod monitor;
pub mod signal;

pub(crate) mod channel;

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
    pub use crate::error::{ExitCode, RequestError, SendError};
    pub use crate::global_context::GlobalContext;
    pub use crate::signal::{Escalation, Signal};

    #[cfg(feature = "remote")]
    pub use theta_macros::impl_id;
    pub use theta_macros::{Actor, ActorConfig, PersistentActor};
}
