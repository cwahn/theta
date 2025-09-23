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
//! - **Auto-generation**: When using the `#[actor]` macro with a custom `type View`,
//!   and self is `Hash`, a `hash_code()` implementation is automatically generated using `FxHasher`
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

use std::{any::Any, sync::LazyLock};

use theta_flume::{Receiver, Sender};
use uuid::Uuid;

#[cfg(feature = "monitor")]
use crate::base::{BindingError, MonitorError};
use crate::{
    actor::{Actor, ActorId},
    actor_instance::Cont,
    actor_ref::ActorHdl,
    message::Escalation,
    prelude::ActorRef,
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
    serde::{Deserialize, Serialize},
    url::Url,
};

// todo Use concurrent hashmap
/// Global registry of active actor handles indexed by actor ID backed by a DashMap
/// for lock-free concurrent read/write access across monitoring, lookup, and lifecycle paths.
pub static HDLS: LazyLock<dashmap::DashMap<ActorId, ActorHdl>> =
    LazyLock::new(|| dashmap::DashMap::with_capacity(1024));

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
/// # Return
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
    let ident_or_url = ident_or_url.as_ref();

    match Url::parse(ident_or_url) {
        Err(_) => match Uuid::try_parse(ident_or_url) {
            Err(_) => Ok(monitor_local::<A>(ident_or_url, tx)?),
            Ok(actor_id) => match LocalPeer::inst().get_imported_peer(&actor_id) {
                None => Ok(monitor_local_id::<A>(actor_id, tx)?),
                Some(peer) => peer.monitor(*actor_id.as_bytes(), tx).await,
            },
        },

        Ok(url) => {
            let (ident, public_key) = split_url(&url)?;
            monitor_remote::<A>(ident, public_key, tx).await
        }
    }
}

// ! Doesn't make sense as in case of remote actor, don't know the public key
// /// Monitor an actor by its unique ID for both local and remote actors.
// ///
// /// This function provides a unified interface for monitoring actors by their
// /// unique ID, automatically determining whether the actor is local or remote
// /// and routing the monitoring request appropriately.
// ///
// /// # Arguments
// ///
// /// * `actor_id` - The unique ID of the actor to observe
// /// * `tx` - Channel to send updates to
// ///
// /// # Return
// ///
// /// `Result<(), RemoteError>` - Success or error during observation setup
// ///
// /// # Errors
// ///
// /// Returns `RemoteError` if:
// /// - Actor with the given ID is not found locally or remotely
// /// - Network connection to remote peer fails
// #[cfg(all(feature = "monitor", feature = "remote"))]
// pub async fn monitor_id<A: Actor>(actor_id: ActorId, tx: UpdateTx<A>) -> Result<(), RemoteError> {
//     // match LocalPeer::inst().get_import::<A>(actor_id) { // ! Not working for non-existing actor
//     //     None => Ok(monitor_local_id::<A>(actor_id, tx)?),
//     //     Some(import) => {
//     //         import
//     //             .peer
//     //             .monitor(actor_id.as_bytes().to_vec().into(), tx)
//     //             .await
//     //     }
//     // }
//     // try to lookup local first, and then try remote if failed
//     match ActorRef::<A>::lookup_local_id(actor_id) {
//         Ok(actor) => Ok(monitor_local_id::<A>(actor.id(), tx)?),
//         Err(_) => {
//             let peer = LocalPeer::inst().get_import_by_id(actor_id)?

//     // let peer = LocalPeer::inst().get_or_connect(public_key)?;
// }

/// Monitor a local actor by name or UUID.
///
/// # Arguments
///
/// * `ident` - Actor name (as bytes) or UUID string
/// * `tx` - Channel to send updates to
///
/// # Return
///
/// `Result<(), MonitorError>` - Success or error during local observation setup
///
/// # Errors
///
/// Returns `MonitorError` if:
/// - Actor not found by the given identifier
/// - Failed to send observation signal to actor
#[cfg(feature = "monitor")]
pub fn monitor_local<A: Actor>(
    ident: impl AsRef<str>,
    tx: UpdateTx<A>,
) -> Result<(), MonitorError> {
    let ident = ident.as_ref();

    match Uuid::try_parse(ident) {
        Err(_) => {
            let actor = ActorRef::<A>::lookup_local(ident)?;
            monitor_local_id::<A>(actor.id(), tx)
        }
        Ok(actor_id) => monitor_local_id::<A>(actor_id, tx),
    }
}

/// Monitor a local actor by its unique ID.
///
/// # Arguments
///
/// * `actor_id` - The unique ID of the actor to observe
/// * `tx` - Channel to send updates to
///
/// # Return
///
/// `Result<(), MonitorError>` - Success or error during observation setup
///
/// # Errors
///
/// Returns `MonitorError` if:
/// - Actor with the given ID is not found
/// - Failed to send observation signal to actor
#[cfg(feature = "monitor")]
pub fn monitor_local_id<A: Actor>(actor_id: ActorId, tx: UpdateTx<A>) -> Result<(), MonitorError> {
    let hdl_entry = HDLS
        .get(&actor_id)
        .ok_or(MonitorError::BindingError(BindingError::NotFound))?;
    let hdl = hdl_entry.value();

    hdl.monitor(Box::new(tx))
        .map_err(|_| MonitorError::SigSendError)?;

    Ok(())
}

/// Monitor a remote actor by identifier and public key.
///
/// # Arguments
///
/// * `ident` - Actor identifier on the remote peer
/// * `public_key` - Public key of the remote peer
/// * `tx` - Channel to send updates to
///
/// # Return
///
/// `Result<(), RemoteError>` - Success or error during remote observation setup
///
/// # Errors
///
/// Returns `RemoteError` if:
/// - Connection to remote peer fails
/// - Remote actor lookup fails
#[cfg(all(feature = "monitor", feature = "remote"))]
pub async fn monitor_remote<A: Actor>(
    ident: Ident,
    public_key: PublicKey,
    tx: UpdateTx<A>,
) -> Result<(), RemoteError> {
    let peer = LocalPeer::inst().get_or_connect_peer(public_key);

    peer.monitor(ident, tx).await
}

/// Monitor a remote actor by its unique ID and public key.
///
/// This function establishes monitoring for a remote actor when you know both
/// the actor's unique ID and the public key of the peer hosting it. It connects
/// to the specified peer and sets up observation of the target actor.
///
/// # Arguments
///
/// * `actor_id` - The unique ID of the remote actor to observe
/// * `public_key` - Public key of the remote peer hosting the actor
/// * `tx` - Channel to send updates to
///
/// # Return
///
/// `Result<(), RemoteError>` - Success or error during remote observation setup
///
/// # Errors
///
/// Returns `RemoteError` if:
/// - Connection to remote peer fails
/// - Remote actor with the given ID is not found
/// - Network communication errors occur
#[cfg(all(feature = "remote", feature = "remote"))]
pub async fn monitor_remote_id<A: Actor>(
    actor_id: ActorId,
    public_key: PublicKey,
    tx: UpdateTx<A>,
) -> Result<(), RemoteError> {
    let peer = LocalPeer::inst().get_or_connect_peer(public_key);

    peer.monitor(*actor_id.as_bytes(), tx).await
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
