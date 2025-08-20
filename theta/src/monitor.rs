//! Actor monitoring and observation capabilities.
//!
//! This module provides a comprehensive monitoring system for observing actor
//! state changes, lifecycle events, and behavior patterns. Monitors enable
//! debugging, metrics collection, and reactive programming patterns.
//!
//! # Core Concepts
//!
//! ## Monitors
//!
//! A **monitor** is an monitor that receives notifications about actor activity.
//! Monitors can monitor:
//! - **State changes**: When actors modify their internal state
//! - **Lifecycle events**: Start, stop, restart, and error conditions
//! - **Message processing**: Timing and throughput metrics
//! - **Remote activity**: Cross-network actor communication
//!
//! ## Updates
//!
//! **Updates** are structured data sent from actors to monitors. Two main types:
//!
//! ### State Updates (`Update::State`)
//! - Contain snapshots of actor state at observation time
//! - Generated from the actor's `View` associated type
//! - Automatically created via `From<&Actor>` implementation
//! - Used for debugging, metrics, and state visualization
//!
//! ### Status Updates (`Update::Status`)
//! - Contain lifecycle and operational information
//! - Include timing data, message counts, and error states
//! - Generated automatically by the framework
//! - Used for health monitoring and performance analysis
//!
//! ## Hashing and Optimization
//!
//! ### State Hash Optimization
//! Actors can implement custom `hash_code()` methods to optimize monitoring:
//! - **Efficient change detection**: Only send updates when hash changes
//!
//! ```ignore
//! impl Actor for MyActor {
//!     fn hash_code(&self) -> u64 {
//!         // Custom hash based on significant state changes
//!         let mut hasher = FxHasher::default();
//!         self.critical_value.hash(&mut hasher);
//!         hasher.finish()
//!     }
//! }
//! ```
//!
//! ### Observation Frequency
//! - **Hash-based**: Update only when state hash changes (most efficient)
//!
//! # Usage Patterns
//!
//! ## Local Actor Observation
//!
//! ```ignore
//! use theta::prelude::*;
//! use theta_flume::unbounded_anonymous;
//!
//! // Set up monitoring channel
//! let (tx, mut rx) = unbounded_anonymous();
//!
//! // Start observing an actor by name
//! monitor::<MyActor>("my_actor", tx).await?;
//!
//! // Process incoming updates
//! while let Some(update) = rx.recv().await {
//!     match update {
//!         Update::State(state) => {
//!             println!("State changed: {state:?}");
//!         },
//!         Update::Status(status) => {
//!             println!("Status: {} messages processed", status.msg_count);
//!         },
//!     }
//! }
//! ```
//!
//! ## Remote Actor Observation
//!
//! ```ignore
//! // Monitor actor on remote peer using iroh:// URL
//! let url = "iroh://my_actor@peer_public_key";
//! monitor::<MyActor>(url, tx).await?;
//! ```
//!
//! ## Custom State Updateing
//!
//! ```ignore
//! #[derive(Debug, Clone, Serialize, Deserialize)]
//! struct DatabaseStats {
//!     active_connections: u32,
//!     query_count: u64,
//!     avg_response_time: Duration,
//! }
//!
//! impl Actor for DatabaseActor {
//!     type View = DatabaseStats;
//!     
//!     fn state_update(&self) -> DatabaseStats {
//!         DatabaseStats {
//!             active_connections: self.pool.active_count(),
//!             query_count: self.metrics.total_queries,
//!             avg_response_time: self.metrics.avg_response_time(),
//!         }
//!     }
//!     
//!     fn hash_code(&self) -> u64 {
//!         // Only update when significant metrics change
//!         (self.metrics.total_queries / 100) ^ self.pool.active_count() as u64
//!     }
//! }
//! ```

use std::{
    any::Any,
    sync::{LazyLock, RwLock},
};

use rustc_hash::FxHashMap;
#[cfg(feature = "remote")]
use serde::{Deserialize, Serialize};
use theta_flume::{Receiver, Sender};
use uuid::Uuid;

use crate::{
    actor::{Actor, ActorId},
    actor_instance::Cont,
    actor_ref::ActorHdl,
    context::{LookupError, MonitorError, RootContext},
    message::Escalation,
};

#[cfg(feature = "remote")]
use {
    crate::{
        base::Ident,
        remote::{
            base::{RemoteError, split_url},
            peer::LocalPeer,
        },
    },
    iroh::PublicKey,
    url::Url,
};

// todo Separate this module as "monitor" feature

/// Global registry of active actor handles indexed by actor ID.
pub static HDLS: LazyLock<RwLock<FxHashMap<ActorId, ActorHdl>>> =
    LazyLock::new(|| RwLock::new(FxHashMap::default()));

/// Type-erased update transmitter for internal use.
pub type AnyUpdateTx = Box<dyn Any + Send>;

/// Channel for sending actor updates to monitors.
pub type UpdateTx<A> = Sender<Update<A>>;

/// Channel for receiving actor updates from observations.
pub type UpdateRx<A> = Receiver<Update<A>>;

/// Internal monitor structure for managing monitors of an actor.
pub(crate) struct Monitor<A: Actor> {
    pub(crate) monitors: Vec<UpdateTx<A>>,
}

/// Updates sent from actors to monitors containing state and status information.
///
/// `Update` is the primary data structure for actor monitoring. It contains
/// either a state snapshot or lifecycle status information about an actor.
///
/// # Variants
///
/// - `State(A::View)` - Contains actor state data for debugging and metrics
/// - `Status(Status)` - Contains lifecycle and operational status information
///
/// # Usage
///
/// Updates are automatically generated by the framework and sent to registered
/// monitors. The frequency and content depend on the actor's configuration:
///
/// ```ignore
/// while let Some(update) = monitor_rx.recv().await {
///     match update {
///         Update::State(state) => {
///             // Process state data for metrics/debugging
///             println!("Actor state: {state:?}");
///         },
///         Update::Status(status) => {
///             // Handle lifecycle events
///             match status {
///                 Status::Processing => println!("Actor is healthy"),
///                 Status::Panic(escalation) => println!("Actor failed: {escalation:?}"),
///                 _ => {}
///             }
///         },
///     }
/// }
/// ```
#[derive(Debug)]
#[cfg_attr(feature = "remote", derive(Serialize, Deserialize))]
pub enum Update<A: Actor> {
    /// Actor state snapshot
    State(A::View),
    /// Actor lifecycle status
    Status(Status),
}

/// Actor lifecycle and processing status information.
///
/// `Status` represents the current operational state of an actor, including
/// both normal operations and exceptional conditions. This information is
/// essential for monitoring actor health and debugging issues.
///
/// # Lifecycle States
///
/// ## Normal Operations
/// - `Processing` - Actor is actively handling messages
/// - `Paused` - Actor is temporarily suspended
/// - `WaitingSignal` - Actor is idle, waiting for new messages
/// - `Resuming` - Actor is transitioning from paused to active state
///
/// ## Supervision States  
/// - `Supervising(ActorId, Escalation)` - Actor is handling a child failure
/// - `CleanupChildren` - Actor is cleaning up terminated child references
///
/// ## Error States
/// - `Panic(Escalation)` - Actor has panicked and is updateing the error
/// - `Restarting` - Actor is being restarted after a failure
/// - `Terminating` - Actor is shutting down gracefully
/// - `Terminated` - Actor has completed shutdown
///
/// # Usage in Monitoring
///
/// Status information is automatically generated during actor lifecycle events:
///
/// ```ignore
/// match status {
///     Status::Processing => {
///         // Actor is healthy and processing messages
///         metrics.record_healthy_actor();
///     },
///     Status::Panic(escalation) => {
///         // Actor has failed, may need intervention
///         alerts.send_failure_notification(escalation);
///     },
///     Status::Supervising(child_id, escalation) => {
///         // Actor is handling child failure
///         supervision_log.record_escalation(child_id, escalation);
///     },
///     _ => {}
/// }
/// ```
#[derive(Debug, Clone)]
#[cfg_attr(feature = "remote", derive(Serialize, Deserialize))]
pub enum Status {
    /// Actor is processing messages
    Processing,
    /// Actor is paused
    Paused,
    /// Actor is waiting for signals
    WaitingSignal,
    /// Actor is resuming from pause
    Resuming,

    /// Actor is supervising a failed child
    Supervising(ActorId, Escalation),
    /// Actor is cleaning up child references
    CleanupChildren,

    /// Actor has panicked
    Panic(Escalation),
    /// Actor is restarting
    Restarting,

    /// Actor is being dropped
    Dropping,
    /// Actor is terminating
    Terminating,
}

/// Monitor an actor by name or URL for both local and remote actors.
///
/// # Arguments
///
/// * `ident_or_url` - Actor name (local) or iroh:// URL (remote)
/// * `tx` - Channel to send updates to
///
/// # Returns
///
/// `Result<(), RemoteError>` - Success or error during observation setup
///
/// # Errors
///
/// Returns `RemoteError` if:
/// - URL parsing fails for remote actors
/// - Network connection to remote peer fails
/// - Actor lookup fails
#[cfg(feature = "remote")]
pub async fn monitor<A: Actor>(
    ident_or_url: impl AsRef<str>,
    tx: UpdateTx<A>,
) -> Result<(), RemoteError> {
    match Url::parse(ident_or_url.as_ref()) {
        Ok(url) => {
            let (ident, public_key) = split_url(&url)?;
            monitor_remote::<A>(ident, public_key, tx).await
        }
        Err(_) => match ident_or_url.as_ref().parse::<Uuid>() {
            Ok(actor_id) => match LocalPeer::inst().get_import::<A>(actor_id) {
                Some(import) => {
                    monitor_remote::<A>(
                        actor_id.as_bytes().to_vec().into(),
                        import.peer.public_key(),
                        tx,
                    )
                    .await
                }
                None => Ok(monitor_local_id::<A>(actor_id, tx)?),
            },
            Err(_) => {
                let ident = ident_or_url.as_ref().as_bytes();
                Ok(monitor_local::<A>(ident, tx)?)
            }
        },
    }
}

// /// Monitor a local actor by name or ID when remote feature is not available.
// ///
// /// # Arguments
// ///
// /// * `ident` - Actor name or UUID string
// /// * `tx` - Channel to send updates to
// ///
// /// # Returns
// ///
// /// `Result<(), MonitorError>` - Success or error during observation setup
// ///
// /// # Errors
// ///
// /// Returns `MonitorError` if:
// /// - Actor with the given name/ID is not found
// /// - Failed to send observation signal to actor
// #[cfg(all(feature = "monitor", not(feature = "remote")))]
// pub fn observe_local_actor<A: Actor>(
//     ident: impl AsRef<str>,
//     tx: UpdateTx<A>,
// ) -> Result<(), MonitorError> {
//     match ident.as_ref().parse::<Uuid>() {
//         Ok(uuid) => observe_local_id::<A>(uuid, tx),
//         Err(_) => {
//             let ident_bytes = ident.as_ref().as_bytes();
//             observe_local::<A>(ident_bytes, tx)
//         }
//     }
// }

/// Monitor a remote actor by identifier and public key.
///
/// # Arguments
///
/// * `ident` - Actor identifier on the remote peer
/// * `public_key` - Public key of the remote peer
/// * `tx` - Channel to send updates to
///
/// # Returns
///
/// `Result<(), RemoteError>` - Success or error during remote observation setup
///
/// # Errors
///
/// Returns `RemoteError` if:
/// - Connection to remote peer fails
/// - Remote actor lookup fails
#[cfg(feature = "remote")]
pub async fn monitor_remote<A: Actor>(
    ident: Ident,
    public_key: PublicKey,
    tx: UpdateTx<A>,
) -> Result<(), RemoteError> {
    let peer = LocalPeer::inst().get_or_connect(public_key)?;

    peer.monitor(ident, tx).await
}

/// Monitor a local actor by name or UUID.
///
/// # Arguments
///
/// * `ident` - Actor name (as bytes) or UUID string
/// * `tx` - Channel to send updates to
///
/// # Returns
///
/// `Result<(), MonitorError>` - Success or error during local observation setup
///
/// # Errors
///
/// Returns `MonitorError` if:
/// - Actor not found by the given identifier
/// - Failed to send observation signal to actor
pub fn monitor_local<A: Actor>(
    ident: impl AsRef<[u8]>,
    tx: UpdateTx<A>,
) -> Result<(), MonitorError> {
    match Uuid::from_slice(ident.as_ref()) {
        Ok(actor_id) => monitor_local_id::<A>(actor_id, tx),
        Err(_) => {
            let actor = RootContext::lookup_local_impl::<A>(ident.as_ref())?;
            monitor_local_id::<A>(actor.id(), tx)
        }
    }
}

/// Monitor a local actor by its unique ID.
///
/// # Arguments
///
/// * `actor_id` - The unique ID of the actor to observe
/// * `tx` - Channel to send updates to
///
/// # Returns
///
/// `Result<(), MonitorError>` - Success or error during observation setup
///
/// # Errors
///
/// Returns `MonitorError` if:
/// - Actor with the given ID is not found
/// - Failed to send observation signal to actor
pub fn monitor_local_id<A: Actor>(actor_id: ActorId, tx: UpdateTx<A>) -> Result<(), MonitorError> {
    let hdls = HDLS.read().unwrap();
    let hdl = hdls
        .get(&actor_id)
        .ok_or(MonitorError::LookupError(LookupError::NotFound))?;

    hdl.monitor(Box::new(tx))
        .map_err(|_| MonitorError::SigSendError)?;

    Ok(())
}

// Implementations

impl<A: Actor> Monitor<A> {
    pub fn add_monitor(&mut self, tx: UpdateTx<A>) {
        self.monitors.push(tx);
    }

    pub fn update(&mut self, update: Update<A>) {
        self.monitors.retain(|tx| tx.send(update.clone()).is_ok());
    }

    pub fn is_monitor(&self) -> bool {
        !self.monitors.is_empty()
    }
}

impl<A: Actor> Default for Monitor<A> {
    fn default() -> Self {
        Monitor {
            monitors: Vec::new(),
        }
    }
}

impl<A: Actor> Clone for Update<A> {
    fn clone(&self) -> Self {
        match self {
            Update::State(state) => Update::State(state.clone()),
            Update::Status(status) => Update::Status(status.clone()),
        }
    }
}

impl From<&Cont> for Status {
    fn from(cont: &Cont) -> Self {
        match cont {
            Cont::Process => Status::Processing,

            Cont::Pause(_) => Status::Paused,
            Cont::WaitSignal => Status::WaitingSignal,
            Cont::Resume(_) => Status::Resuming,

            // ? Do I need ActorId here?
            Cont::Supervise(_, e) => Status::Supervising(ActorId::nil(), e.clone()),
            Cont::CleanupChildren => Status::CleanupChildren,

            Cont::Panic(e) => Status::Panic(e.clone()),
            Cont::Restart(_) => Status::Restarting,

            Cont::Drop => Status::Dropping,
            Cont::Terminate(_) => Status::Terminating,
        }
    }
}
