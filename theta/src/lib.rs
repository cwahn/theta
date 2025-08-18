//! # Theta: An Async Actor Framework for Rust
//!
//! Theta is an **ergonomic** yet **minimal** and **performant** async actor framework for Rust.
//!
//! ## Key Features
//!
//! - **Async-first**: Built on top of `tokio`, actors are lightweight tasks with MPSC communication
//! - **Built-in remote capabilities**: Distributed actor systems powered by P2P protocol ([iroh])
//! - **Built-in monitoring**: Observe actor state changes and lifecycle events
//! - **Built-in persistence**: Seamless actor snapshots and recovery
//! - **Type-safe messaging**: Compile-time guarantees for message handling
//! - **Ergonomic macros**: Simplified actor definition with the `#[actor]` attribute
//!
//! ## Quick Start
//!
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

// Re-exports for convenience
pub use actor::{Actor, ActorArgs, ActorId, ExitCode};
pub use actor_ref::{ActorRef, WeakActorRef};
pub use base::{Ident, Nil};
pub use context::{Context, RootContext};
pub use message::{Message, Signal};

#[cfg(feature = "monitor")]
pub use monitor::{observe, observe_local, observe_local_id, Report, ReportRx, ReportTx, Status};

#[cfg(feature = "persistence")]
pub use persistence::{PersistentActor, PersistentSpawnExt, PersistentStorage, SaveSnapshotExt};

#[cfg(feature = "remote")]
pub use remote::base::{RemoteError, Tag};

/// The prelude module re-exports the most commonly used types and traits.
///
/// This module contains all the essential types needed for basic actor usage.
/// Most applications should use `use theta::prelude::*;` to import these items.
pub mod prelude {
    // Core actor types
    pub use crate::{
        actor::{Actor, ActorArgs, ActorId},
        actor_ref::{ActorRef, WeakActorRef},
        base::{Ident, Nil},
        context::{Context, RootContext},
        message::{Message, Signal},
    };

    // Monitoring types (when enabled)
    #[cfg(feature = "monitor")]
    pub use crate::monitor::{observe, Report, Status};

    // Persistence types (when enabled)
    #[cfg(feature = "persistence")]
    pub use crate::persistence::{PersistentActor, PersistentSpawnExt, PersistentStorage};

    // Remote types (when enabled)
    #[cfg(feature = "remote")]
    pub use crate::remote::base::Tag;

    // Macros
    pub use theta_macros::{ActorArgs, actor};
}
