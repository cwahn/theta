use std::sync::{Arc, LazyLock, Mutex};

use dashmap::DashMap;
use rustc_hash::FxBuildHasher;
use serde::{Deserialize, Serialize};
use theta_flume::unbounded_with_id;
use thiserror::Error;
use tokio::sync::Notify;
use tracing::{debug, error, trace};
use uuid::Uuid;

use crate::{
    actor::{Actor, ActorArgs, ActorId},
    actor_instance::ActorConfig,
    actor_ref::{ActorHdl, ActorRef, AnyActorRef, WeakActorHdl, WeakActorRef},
    base::Ident,
    message::RawSignal,
};

#[cfg(feature = "monitor")]
use crate::monitor::HDLS;

#[cfg(feature = "remote")]
use {
    crate::remote::{base::ActorTypeId, peer::LocalPeer},
    iroh::PublicKey,
};

// todo Use concurrent hashmap
/// Global registry mapping identifiers to actor references for named bindings.
/// Replaced with DashMap for reduced contention on high lookup concurrency.
pub(crate) static BINDINGS: LazyLock<DashMap<Ident, Arc<dyn AnyActorRef>, FxBuildHasher>> =
    LazyLock::new(DashMap::default);

/// Actor execution context providing communication and spawning capabilities.
///
/// The `Context` is passed to actor methods and provides access to:
/// - Self reference for sending messages to itself via `ctx.this.upgrade()`
/// - Child actor management via `ctx.spawn()`
/// - Actor metadata via `ctx.id()`
/// - Lifecycle control via `ctx.terminate()`
///
/// # Type Parameters
///
/// * `A` - The actor type this context belongs to
#[derive(Debug, Clone)]
pub struct Context<A: Actor> {
    /// Weak reference to this actor instance
    pub this: WeakActorRef<A>,
    pub(crate) this_hdl: ActorHdl, // Self supervision reference
    pub(crate) child_hdls: Arc<Mutex<Vec<WeakActorHdl>>>, // children of this actor
}

/// Root context for the actor system.
///
/// `RootContext` serves as the entry point for the actor system and provides
/// methods for:
/// - Initializing the actor system (local or remote)
/// - Spawning top-level actors
/// - Binding actors to names for lookup
/// - Looking up remote actors (with `remote` feature)
///
/// # Examples
///
/// ```
/// use theta::prelude::*;
///
/// #[derive(Debug, Clone, ActorArgs)]
/// struct MyActor { value: i32 }
///
/// #[actor("12345678-1234-5678-9abc-123456789abc")]
/// impl Actor for MyActor {}
///
/// // Compile-only example
/// fn example() {
///     let ctx = RootContext::init_local();
///     let actor = ctx.spawn(MyActor { value: 0 });
///     ctx.bind(b"my_actor", actor);
/// }
/// ```
#[derive(Debug, Clone)]
pub struct RootContext {
    pub(crate) this_hdl: ActorHdl,                        // Self reference
    pub(crate) child_hdls: Arc<Mutex<Vec<WeakActorHdl>>>, // children of the global context
}

/// Errors that can occur during actor lookup operations.
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum LookupError {
    #[error("actor not found")]
    NotFound,
    #[error("actor type id mismatch")]
    TypeMismatch,
    #[error("actor ref downcast failed")]
    DowncastError,
    #[cfg(feature = "remote")]
    #[error(transparent)]
    SerializeError(#[from] postcard::Error),
}

// Implementations

impl<A: Actor> Context<A> {
    /// Get the unique identifier of this actor.
    ///
    /// # Returns
    ///
    /// `ActorId` uniquely identifying this actor instance.
    pub fn id(&self) -> ActorId {
        self.this_hdl.id()
    }

    /// Spawn a new child actor with the given arguments.
    ///
    /// # Arguments
    ///
    /// * `args` - Initialization arguments for the new actor
    ///
    /// # Returns
    ///
    /// `ActorRef<Args::Actor>` reference to the newly spawned actor.
    pub fn spawn<Args: ActorArgs>(&self, args: Args) -> ActorRef<Args::Actor> {
        let (hdl, actor) = spawn_impl(&self.this_hdl, args);

        self.child_hdls.lock().unwrap().push(hdl.downgrade());

        actor
    }

    /// Terminate this actor and all its children.
    // todo Make it support timeout
    pub async fn terminate(&self) {
        let k = Arc::new(Notify::new());
        self.this_hdl
            .raw_send(RawSignal::Terminate(Some(k.clone())))
            .unwrap();

        k.notified().await;
    }
}

impl RootContext {
    /// Initialize the root context with remote capabilities.
    ///
    /// # Arguments
    ///
    /// * `endpoint` - The iroh network endpoint for remote communication
    ///
    /// # Returns
    ///
    /// `RootContext` with remote communication enabled.
    #[cfg(feature = "remote")]
    pub fn init(endpoint: iroh::Endpoint) -> Self {
        LocalPeer::init(endpoint);

        Self::init_local()
    }

    /// Initialize a local-only root context.
    ///
    /// # Returns
    ///
    /// `RootContext` for local actor system operations.
    pub fn init_local() -> Self {
        Self::default()
    }

    /// Get the public key of this node for remote communication.
    ///
    /// # Returns
    ///
    /// `PublicKey` for this node's identity in remote operations.
    #[cfg(feature = "remote")]
    pub fn public_key(&self) -> PublicKey {
        LocalPeer::inst().public_key()
    }

    /// Spawn a new top-level actor.
    ///
    /// # Arguments
    ///
    /// * `args` - The initialization arguments for the new actor
    ///
    /// # Returns
    ///
    /// `ActorRef<Args::Actor>` reference to the newly spawned actor.
    pub fn spawn<Args: ActorArgs>(&self, args: Args) -> ActorRef<Args::Actor> {
        let (actor_hdl, actor) = spawn_impl(&self.this_hdl, args);

        self.child_hdls.lock().unwrap().push(actor_hdl.downgrade());

        actor
    }

    /// Bind an actor to a global identifier for lookup.
    ///
    /// # Arguments
    ///
    /// * `ident` - The identifier to bind the actor to
    /// * `actor` - The actor reference to bind
    pub fn bind<A: Actor>(&self, ident: impl Into<Ident>, actor: ActorRef<A>) {
        let ident = ident.into();
        trace!(?ident, %actor, "binding");
        Self::bind_impl(ident, actor);
    }

    /// Remove an actor binding from the global registry.
    ///
    /// # Arguments
    ///
    /// * `ident` - The identifier to remove from the registry
    ///
    /// # Returns
    ///
    /// `Option<Arc<dyn AnyActorRef>>` - The removed binding if it existed.
    pub fn free(&self, ident: impl AsRef<[u8]>) -> Option<Arc<dyn AnyActorRef>> {
        BINDINGS.remove(ident.as_ref()).map(|(_, v)| v)
    }

    /// Terminate the root context and all spawned actors.
    ///
    /// Initiates system-wide shutdown and waits for all actors to terminate.
    ///
    /// # Note
    ///
    /// TODO: Make it support timeout for graceful shutdown with fallback to forced termination.
    // todo Make it support timeout
    pub async fn terminate(&self) {
        let k = Arc::new(Notify::new());
        self.this_hdl
            .raw_send(RawSignal::Terminate(Some(k.clone())))
            .unwrap();

        k.notified().await;
    }

    #[allow(dead_code)]
    pub(crate) fn is_bound_impl<A: Actor>(ident: impl AsRef<[u8]>) -> bool {
        let Some(actor) = BINDINGS.get(ident.as_ref()) else {
            return false;
        };

        actor.as_any().is::<ActorRef<A>>()
    }

    pub(crate) fn bind_impl<A: Actor>(ident: Ident, actor: ActorRef<A>) {
        let _ = BINDINGS.insert(ident, Arc::new(actor));
    }

    #[cfg(feature = "remote")]
    pub(crate) fn lookup_any_local(
        actor_ty_id: ActorTypeId,
        ident: impl AsRef<[u8]>,
    ) -> Result<Arc<dyn AnyActorRef>, LookupError> {
        Self::lookup_any_local_unchecked(ident).and_then(|actor| {
            if actor.ty_id() == actor_ty_id {
                Ok(actor)
            } else {
                Err(LookupError::TypeMismatch)
            }
        })
    }

    #[cfg(feature = "remote")]
    pub(crate) fn lookup_any_local_unchecked(
        ident: impl AsRef<[u8]>,
    ) -> Result<Arc<dyn AnyActorRef>, LookupError> {
        let Some(actor) = BINDINGS.get(ident.as_ref()) else {
            return Err(LookupError::NotFound);
        };

        Ok(actor.clone())
    }
}

impl Default for RootContext {
    fn default() -> Self {
        let (sig_tx, sig_rx) = unbounded_with_id(Uuid::new_v4());
        let this_hdl = ActorHdl(sig_tx);
        let child_hdls = Arc::new(Mutex::new(Vec::<WeakActorHdl>::new()));

        tokio::spawn({
            let child_hdls = child_hdls.clone();

            // Minimal supervision logic
            async move {
                while let Some(sig) = sig_rx.recv().await {
                    match sig {
                        RawSignal::Escalation(child_hdl, esc) => {
                            error!("received escalation from actor {}: {esc:?}", child_hdl.id());
                            child_hdl.raw_send(RawSignal::Terminate(None)).unwrap();
                        }
                        RawSignal::ChildDropped => {
                            debug!("a top-level actor has been dropped");

                            let mut child_hdls = child_hdls.lock().unwrap();
                            child_hdls.retain(|hdl| hdl.0.strong_count() > 0);
                        }
                        _ => unreachable!(
                            "Escalation and ChildDropped are the only signals expected"
                        ),
                    }
                }
            }
        });

        Self {
            this_hdl,
            child_hdls,
        }
    }
}

/// Create a new actor instance with auto-generated ID.
pub(crate) fn spawn_impl<Args: ActorArgs>(
    parent_hdl: &ActorHdl,
    args: Args,
) -> (ActorHdl, ActorRef<Args::Actor>) {
    spawn_with_id_impl(Uuid::new_v4(), parent_hdl, args)
}

/// Create a new actor instance with specified ID.
pub(crate) fn spawn_with_id_impl<Args: ActorArgs>(
    actor_id: ActorId,
    parent_hdl: &ActorHdl,
    args: Args,
) -> (ActorHdl, ActorRef<Args::Actor>) {
    let (msg_tx, msg_rx) = unbounded_with_id(actor_id);
    let (sig_tx, sig_rx) = unbounded_with_id(actor_id);

    let actor_hdl = ActorHdl(sig_tx);
    let actor = ActorRef(msg_tx);

    // Ignore chance of UUID v4 collision
    #[cfg(feature = "monitor")]
    let _ = HDLS.insert(actor_id, actor_hdl.clone());

    tokio::spawn({
        let actor = actor.downgrade();
        let parent_hdl = parent_hdl.clone();
        let actor_hdl = actor_hdl.clone();

        async move {
            let config = ActorConfig::new(actor, parent_hdl, actor_hdl, sig_rx, msg_rx, args);
            config.exec().await;

            #[cfg(feature = "monitor")]
            let _ = HDLS.remove(&actor_id);
        }
    });

    (actor_hdl, actor)
}
