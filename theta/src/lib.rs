extern crate self as theta;

pub mod actor;
pub mod actor_instance;
pub mod actor_ref;
pub mod base;
pub mod context;
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
    pub use crate::{
        actor::Actor,
        actor_ref::{ActorRef, WeakActorRef},
        base::{Ident, Nil},
        context::{Context, RootContext},
        message::{Message, Signal},
    };

    pub use theta_macros::{ActorArgs, PersistentActor, actor};

    #[cfg(feature = "remote")]
    pub use crate::remote::base::Tag;
}
