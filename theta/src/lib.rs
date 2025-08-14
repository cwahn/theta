extern crate self as theta;

pub mod actor;
pub mod actor_instance;
pub mod actor_ref;
pub mod base;
pub mod context;
pub mod message;
pub mod monitor;

#[cfg(feature = "persistence")]
pub mod persistence;
#[cfg(feature = "remote")]
pub mod remote;

#[cfg(test)]
pub mod dev;

// Re-exports

pub mod prelude {
    pub use crate::actor::Actor;
    pub use crate::actor_ref::{ActorRef, WeakActorRef};
    pub use crate::context::{Context, RootContext};
    pub use crate::message::{Escalation, Signal, Message};

    #[cfg(feature = "persistence")]
    pub use crate::persistence::{PersistentActor, ContextExt as PersistenceContextExt};

    pub use theta_macros::{ActorArgs, PersistentActor, actor};
}
