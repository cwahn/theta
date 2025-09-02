//! # Theta: An Async Actor Framework for Rust
//!
//! Theta is an **ergonomic** yet **minimal** and **performant** async actor framework for Rust.
//!
//! ## Key Features
//!
//! - **Async-first**: Built on top of `tokio`, actors are lightweight tasks with MPMC communication
//! - **Built-in remote capabilities**: Distributed actor systems powered by P2P protocol ([iroh])
//! - **Built-in monitoring**: Monitor actor state changes and lifecycle events
//! - **Built-in persistence**: Seamless actor snapshots and recovery
//! - **Type-safe messaging**: Compile-time guarantees for message handling
//! - **Ergonomic macros**: Simplified actor definition with the `#[actor]` attribute
//!
//! ## Quick Start
//!
//! #[cfg(feature = "full")]
//! ```rust
//! use serde::{Deserialize, Serialize};
//! use theta::prelude::*;
//!
//! // Define actor state
//! #[derive(Debug, Clone, ActorArgs)]
//! struct Counter { value: i64 }
//!
//! // Define messages
//! #[derive(Debug, Clone, Serialize, Deserialize)]
//! struct Inc(i64);
//!
//! #[derive(Debug, Clone, Serialize, Deserialize)]
//! struct GetValue;
//!
//! // Implement actor behavior
//! #[actor("96d9901f-24fc-4d82-8eb8-023153d41074")]
//! impl Actor for Counter {
//!     const _: () = {
//!         async |Inc(amount): Inc| {
//!             self.value += amount;
//!         };
//!
//!         async |_: GetValue| -> i64 {
//!             self.value
//!         };
//!     };
//! }
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let ctx = RootContext::init_local();
//!     let counter = ctx.spawn(Counter { value: 0 });
//!
//!     counter.tell(Inc(5))?; // Fire-and-forget
//!     let current = counter.ask(GetValue).await?; // Request-response
//!     println!("Current value: {current}"); // Current value: 5
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Features
//!
//! - **`macros`** (default): Enables the `#[actor]` and `ActorArgs` derive macros
//! - **`remote`**: Enables distributed actor systems via P2P networking
//! - **`monitor`**: Enables actor state monitoring and observation
//! - **`persistence`**: Enables actor state persistence and recovery
//!
//! [iroh]: https://iroh.computer/

extern crate self as theta;

pub mod actor;
pub mod actor_ref;
#[macro_use]
pub mod base;
pub mod context;
pub mod message;
#[cfg(feature = "monitor")]
pub mod monitor;

#[cfg(feature = "remote")]
pub mod remote;

#[cfg(feature = "persistence")]
pub mod persistence;

pub(crate) mod actor_instance;
#[cfg(test)]
mod dev;

pub(crate) use theta_macros::{debug, error, trace};
#[allow(unused_imports)]
pub(crate) use theta_macros::{info, warn};

/// The prelude module re-exports the most commonly used types and traits.
///
/// This module contains all the essential types needed for basic actor usage.
/// Most applications should use `use theta::prelude::*;` to import these items.
pub mod prelude {
    // Core actor types
    pub use crate::{
        actor::{Actor, ActorArgs, ActorId, ExitCode},
        actor_ref::{ActorRef, WeakActorRef},
        base::{Ident, Nil},
        context::{Context, RootContext},
        message::{Message, Signal},
    };

    // Monitoring types (when enabled)
    #[cfg(feature = "monitor")]
    pub use crate::monitor::{Status, Update, UpdateRx, UpdateTx, monitor_local, monitor_local_id};

    // Persistence types (when enabled)
    #[cfg(feature = "persistence")]
    pub use crate::persistence::{
        PersistentActor, PersistentSpawnExt, PersistentStorage, SaveSnapshotExt,
    };

    // Remote types (when enabled)
    #[cfg(feature = "remote")]
    pub use crate::remote::base::{RemoteError, Tag};

    #[cfg(all(feature = "remote", feature = "monitor"))]
    pub use crate::monitor::monitor;

    // Macros
    #[cfg(feature = "macros")]
    pub use theta_macros::{ActorArgs, actor};
}

/// Private re-exports for macro use. Do not use directly.
#[doc(hidden)]
pub mod __private {
    pub use log;
    pub use rustc_hash;
    pub use serde;
    pub use uuid;

    #[cfg(feature = "remote")]
    pub use postcard;
}
