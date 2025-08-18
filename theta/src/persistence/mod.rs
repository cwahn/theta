//! Actor persistence capabilities for state snapshots and recovery.
//!
//! This module enables actors to persist their state and recover from failures
//! or system restarts. It provides a flexible architecture that supports
//! multiple storage backends and automatic snapshot management.
//!
//! # Core Concepts
//!
//! ## Snapshots
//!
//! A **snapshot** is a serializable representation of an actor's state at a point in time.
//! Snapshots must:
//! - Capture all essential state needed to restore the actor
//! - Be serializable (implement `Serialize` + `Deserialize`)
//! - Be usable as actor initialization arguments (implement `ActorArgs`)
//! - Convert from the actor's current state (implement `From<&Actor>`)
//!
//! ## Storage Backends
//!
//! Storage backends implement the `PersistentStorage` trait and handle:
//! - Reading snapshots by actor ID
//! - Writing snapshots atomically
//! - Storage-specific optimizations (compression, indexing, etc.)
//!
//! Built-in storage backends:
//! - **File system**: Local disk storage with configurable directories
//! - **Memory**: In-memory storage for testing and temporary persistence
//!
//! ## Persistence Lifecycle
//!
//! 1. **Snapshot Creation**: Actors create snapshots from their current state
//! 2. **Storage**: Snapshots are written to the configured storage backend
//! 3. **Recovery**: Actors are restored from snapshots during spawn operations
//! 4. **Automatic Management**: Framework handles snapshot timing and cleanup
//!
//! # Usage Patterns
//!
//! ## Basic Persistence
//!
//! ```ignore
//! use theta::prelude::*;
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Debug, Clone, Serialize, Deserialize, ActorArgs)]
//! struct Counter {
//!     value: i64,
//!     name: String,
//! }
//!
//! // The `snapshot` flag automatically implements PersistentActor
//! #[actor("12345678-1234-5678-9abc-123456789abc", snapshot)]
//! impl Actor for Counter {
//!     const _: () = {
//!         async |Increment(amount): Increment| {
//!             self.value += amount;
//!             // Snapshot is automatically saved periodically
//!         };
//!     };
//! }
//! ```
//!
//! ## Custom Snapshot Types
//!
//! ```ignore
//! #[derive(Debug, Clone, Serialize, Deserialize)]
//! struct CounterSnapshot {
//!     value: i64,
//!     // Exclude non-essential state like caches
//! }
//!
//! impl From<&Counter> for CounterSnapshot {
//!     fn from(counter: &Counter) -> Self {
//!         Self { value: counter.value }
//!     }
//! }
//!
//! impl ActorArgs for CounterSnapshot {
//!     type Actor = Counter;
//!     
//!     async fn initialize(ctx: Context<Counter>, args: &Self) -> Counter {
//!         Counter {
//!             value: args.value,
//!             cache: HashMap::new(), // Rebuilt on recovery
//!         }
//!     }
//! }
//! ```
//!
//! ## Recovery-aware Spawning
//!
//! ```ignore
//! // Try to restore from snapshot, fallback to new instance
//! let counter = ctx.spawn_persistent_or_fallback(
//!     actor_id,
//!     Counter { value: 0, name: "default".to_string() }
//! ).await?;
//! ```

pub mod persistent_actor;
pub mod storages;

// Re-exports
pub use persistent_actor::{
    PersistentActor, PersistentSpawnExt, PersistentStorage, SaveSnapshotExt, PersistenceError,
};
