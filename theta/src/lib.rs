#![no_std]

#[cfg(feature = "std")]
extern crate std;

#[cfg(feature = "alloc")]
extern crate alloc;

extern crate self as theta;

pub mod actor;
pub mod actor_instance;
pub mod actor_ref;
pub mod base;
pub mod error;
pub mod message;
pub mod monitor;

#[cfg(feature = "remote")]
// pub mod remote;
#[cfg(test)]
pub mod dev;
#[cfg(feature = "persistence")]
// pub mod persistence;

// Re-exports

pub mod prelude {
    pub use crate::actor::Actor;
    pub use crate::actor_ref::{ActorRef, WeakActorRef};
    pub use crate::error::{ExitCode, RequestError, SendError};

    #[cfg(feature = "remote")]
    pub use theta_macros::impl_id;
    pub use theta_macros::{Actor, ActorConfig, PersistentActor};
}
