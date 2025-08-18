//! Actor monitoring and observation capabilities.
//!
//! This module provides functionality to observe actor state changes and lifecycle events.
//! Monitors can observe both local and remote actors (with the `remote` feature enabled).
//!
//! # Examples
//!
//! ```
//! use theta::prelude::*;
//! use theta_flume::unbounded_anonymous;
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Debug, Clone, ActorArgs)]
//! struct MyActor { value: i32 }
//!
//! #[actor("12345678-1234-5678-9abc-123456789abc")]
//! impl Actor for MyActor {}
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Initialize context and spawn actor
//!     let ctx = RootContext::init_local();
//!     let actor = ctx.spawn(MyActor { value: 42 });
//!     ctx.bind(b"my_actor", actor);
//!
//!     // Create a channel for receiving reports
//!     let (tx, mut rx) = unbounded_anonymous();
//!
//!     // Observe a local actor by name
//!     observe::<MyActor>("my_actor", tx).await?;
//!
//!     // Receive state reports (just check one)
//!     if let Some(report) = rx.recv().await {
//!         match report {
//!             Report::State(state) => println!("Actor state: {state:?}"),
//!             Report::Status(status) => println!("Actor status: {status:?}"),
//!         }
//!     }
//!     Ok(())
//! }
//! ```

use std::{
    any::Any,
    sync::{LazyLock, RwLock},
};

#[cfg(feature = "remote")]
use iroh::PublicKey;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use theta_flume::{Receiver, Sender};

use crate::{
    actor::{Actor, ActorId},
    actor_instance::Cont,
    actor_ref::ActorHdl,
    context::{LookupError, ObserveError, RootContext},
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
    url::Url,
    uuid::Uuid,
};

// todo Separate this module as "monitor" feature

pub static HDLS: LazyLock<RwLock<FxHashMap<ActorId, ActorHdl>>> =
    LazyLock::new(|| RwLock::new(FxHashMap::default()));

/// Type-erased report transmitter for internal use.
pub type AnyReportTx = Box<dyn Any + Send>;

/// Channel for sending actor reports to observers.
pub type ReportTx<A> = Sender<Report<A>>;

/// Channel for receiving actor reports from observations.
pub type ReportRx<A> = Receiver<Report<A>>;

/// Internal monitor structure for managing observers of an actor.
pub(crate) struct Monitor<A: Actor> {
    pub(crate) observers: Vec<ReportTx<A>>,
}

/// Reports sent by actors to their observers.
///
/// This enum contains different types of information that can be observed
/// about an actor's state and status.
#[derive(Debug)]
#[cfg(feature = "remote")]
#[derive(Serialize, Deserialize)]
pub enum Report<A: Actor> {
    /// Actor state snapshot
    State(A::StateReport),
    /// Actor lifecycle status
    Status(Status),
}

/// Actor lifecycle and processing status.
///
/// This enum represents the current state of an actor in terms of
/// its lifecycle and message processing status.
#[derive(Debug, Clone)]
#[cfg(feature = "remote")]
#[derive(Serialize, Deserialize)]
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

/// Observe an actor by name or URL.
///
/// This function can observe both local and remote actors:
/// - For local actors, pass the actor's name as a string
/// - For remote actors, pass a URL in the format `iroh://name@public_key`
///
/// # Arguments
///
/// * `ident_or_url` - Actor name (local) or iroh:// URL (remote)
/// * `tx` - Channel to send reports to
///
/// # Examples
///
/// ```
/// use theta::prelude::*;
/// use theta_flume::unbounded_anonymous;
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Debug, Clone, ActorArgs)]
/// struct MyActor { value: i32 }
///
/// #[actor("12345678-1234-5678-9abc-123456789abc")]
/// impl Actor for MyActor {}
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     // Initialize context and spawn actor
///     let ctx = RootContext::init_local();
///     let actor = ctx.spawn(MyActor { value: 42 });
///     ctx.bind(b"my_actor", actor);
///
///     let (tx, rx) = unbounded_anonymous();
///     
///     // Observe local actor
///     observe::<MyActor>("my_actor", tx).await?;
///     Ok(())
/// }
/// ```
#[cfg(feature = "remote")]
pub async fn observe<A: Actor>(
    ident_or_url: impl AsRef<str>,
    tx: ReportTx<A>,
) -> Result<(), RemoteError> {
    match Url::parse(ident_or_url.as_ref()) {
        Ok(url) => {
            let (ident, public_key) = split_url(&url)?;
            observe_remote::<A>(ident, public_key, tx).await
        }
        Err(_) => {
            let ident = ident_or_url.as_ref().as_bytes();
            Ok(observe_local::<A>(ident, tx)?)
        }
    }
}

/// Observe a remote actor by identifier and public key.
#[cfg(feature = "remote")]
pub async fn observe_remote<A: Actor>(
    ident: Ident,
    public_key: PublicKey,
    tx: ReportTx<A>,
) -> Result<(), RemoteError> {
    let peer = LocalPeer::inst().get_or_connect(public_key)?;

    peer.observe(ident, tx).await?;

    Ok(())
}

/// Observe a local actor by name or UUID.
///
/// # Arguments
///
/// * `ident` - Actor name (as bytes) or UUID string
/// * `tx` - Channel to send reports to
pub fn observe_local<A: Actor>(
    ident: impl AsRef<[u8]>,
    tx: ReportTx<A>,
) -> Result<(), ObserveError> {
    match Uuid::from_slice(ident.as_ref()) {
        Ok(actor_id) => observe_local_id::<A>(actor_id, tx),
        Err(_) => {
            let actor = RootContext::lookup_any_local_unchecked(ident)?;
            observe_local_id::<A>(actor.id(), tx)
        }
    }
}

/// Observe a local actor by its unique ID.
///
/// # Arguments
///
/// * `actor_id` - The unique ID of the actor to observe
/// * `tx` - Channel to send reports to
pub fn observe_local_id<A: Actor>(actor_id: ActorId, tx: ReportTx<A>) -> Result<(), ObserveError> {
    let hdls = HDLS.read().unwrap();
    let hdl = hdls
        .get(&actor_id)
        .ok_or(ObserveError::LookupError(LookupError::NotFound))?;

    hdl.observe(Box::new(tx))
        .map_err(|_| ObserveError::SigSendError)?;

    Ok(())
}

// Implementations

impl<A: Actor> Monitor<A> {
    pub fn add_observer(&mut self, tx: ReportTx<A>) {
        self.observers.push(tx);
    }

    pub fn report(&mut self, report: Report<A>) {
        self.observers.retain(|tx| tx.send(report.clone()).is_ok());
    }

    pub fn is_observer(&self) -> bool {
        !self.observers.is_empty()
    }
}

impl<A: Actor> Default for Monitor<A> {
    fn default() -> Self {
        Monitor {
            observers: Vec::new(),
        }
    }
}

impl<A: Actor> Clone for Report<A> {
    fn clone(&self) -> Self {
        match self {
            Report::State(state) => Report::State(state.clone()),
            Report::Status(status) => Report::Status(status.clone()),
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
