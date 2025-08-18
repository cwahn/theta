extern crate self as theta;

pub mod actor;
pub mod actor_instance;
pub mod actor_ref;
pub mod base;
pub mod context;
pub mod message;

#[cfg(feature = "monitor")]
pub mod monitor;

#[cfg(feature = "remote")]
pub mod remote;

#[cfg(feature = "persistence")]
pub mod persistence;

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

    #[cfg(feature = "monitor")]
    pub use crate::monitor::observe;

    pub use theta_macros::{ActorArgs, actor};

    #[cfg(feature = "remote")]
    pub use crate::remote::base::Tag;
}
