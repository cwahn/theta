//! Actor persistence capabilities for state snapshots and recovery.
//!
//! This module provides traits and utilities for persisting actor state
//! to storage backends and recovering actors from their snapshots.
//!
//! # Example
//!
//! ```
//! use theta::prelude::*;
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Debug, Clone, Serialize, Deserialize, ActorArgs)]
//! struct MyActor {
//!     value: i32,
//! }
//!
//! #[derive(Debug, Clone, Serialize, Deserialize)]
//! struct Increment(i32);
//!
//! // The `snapshot` flag in #[actor] automatically implements PersistentActor
//! #[actor("12345678-1234-5678-9abc-123456789abc", snapshot)]
//! impl Actor for MyActor {
//!     const _: () = {
//!         async |Increment(value): Increment| {
//!             self.value += value;
//!         };
//!     };
//! }
//! ```

pub mod persistent_actor;
pub mod storages;

// Re-exports
pub use persistent_actor::{
    PersistentActor, PersistentSpawnExt, PersistentStorage, SaveSnapshotExt, PersistenceError,
};
