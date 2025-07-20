pub mod actor;
pub mod actor_instance;
pub mod actor_ref;
pub mod base;
pub mod context;
pub mod error;
pub mod global;
pub mod message;
pub mod weak_actor_ref;

#[cfg(feature = "persistence")]
pub mod persistence;
#[cfg(feature = "remote")]
pub mod remote;

// ! temp
pub mod test;

// Re-exports

pub mod prelude {
    pub use crate::actor::Actor;
    pub use crate::actor_ref::{ActorRef, WeakActorRef};
    pub use crate::context::Context;
    pub use crate::error::{ExitCode, RequestError, SendError};
    pub use crate::global::GlobalContext;
    pub use crate::message::{Behavior, DynMessage, Escalation, Message, Signal};

    pub use theta_macros::PersistentActor;
}
