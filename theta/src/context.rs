use std::{
    any::type_name,
    sync::{Arc, LazyLock, Mutex, RwLock},
};

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use theta_flume::unbounded_with_id;
use thiserror::Error;
use tokio::sync::Notify;
use uuid::Uuid;

use crate::{
    actor::{Actor, ActorArgs, ActorId},
    actor_instance::ActorConfig,
    actor_ref::{ActorHdl, ActorRef, AnyActorRef, WeakActorHdl, WeakActorRef},
    base::Ident,
    debug, error,
    message::RawSignal,
    trace,
};

#[cfg(feature = "monitor")]
use crate::monitor::HDLS;

#[cfg(feature = "remote")]
use {
    crate::remote::{
        base::{ActorTypeId, RemoteError, split_url},
        peer::LocalPeer,
    },
    iroh::PublicKey,
    url::Url,
};

/// Global registry mapping identifiers to actor references for named bindings.
static BINDINGS: LazyLock<RwLock<FxHashMap<Ident, Arc<dyn AnyActorRef>>>> =
    LazyLock::new(|| RwLock::new(FxHashMap::default()));

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
    #[error("actor type mismatch")]
    TypeMismatch,
    #[cfg(feature = "remote")]
    #[error(transparent)]
    SerializeError(#[from] postcard::Error),
}

/// Errors that can occur when monitoring actor status.
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum MonitorError {
    #[error(transparent)]
    LookupError(#[from] LookupError),
    #[error("failed to send signal")]
    SigSendError,
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

    /// Look up an actor by identifier or URL.
    ///
    /// Determines lookup type by URL parsing - if successful performs remote lookup,
    /// otherwise performs local lookup. May establish network connection for URLs.
    #[cfg(feature = "remote")]
    pub async fn lookup<A: Actor>(
        &self,
        ident_or_url: impl AsRef<str>,
    ) -> Result<ActorRef<A>, RemoteError> {
        match Url::parse(ident_or_url.as_ref()) {
            Ok(url) => {
                let (ident, public_key) = split_url(&url)?;
                Ok(self.lookup_remote::<A>(ident, public_key).await??)
            }
            Err(_) => {
                let ident = ident_or_url.as_ref().as_bytes();
                Ok(self.lookup_local::<A>(ident)?)
            }
        }
    }

    /// Look up an actor on a specific remote node.
    ///
    /// # Arguments
    ///
    /// * `ident` - The identifier of the actor on the remote node
    /// * `public_key` - The public key of the target remote node
    ///
    /// # Returns
    ///
    /// - `Ok(Ok(ActorRef<A>))` if the remote actor is found and accessible
    /// - `Ok(Err(LookupError))` if the actor is not found on the remote node
    /// - `Err(RemoteError)` if network communication fails
    ///
    /// # Network Effects
    ///
    /// Establishes network connection to remote peer if not already connected.
    #[cfg(feature = "remote")]
    pub async fn lookup_remote<A: Actor>(
        &self,
        ident: impl Into<Ident>,
        public_key: PublicKey,
    ) -> Result<Result<ActorRef<A>, LookupError>, RemoteError> {
        let peer = LocalPeer::inst().get_or_connect(public_key)?;

        peer.lookup(ident.into()).await
    }

    /// Look up an actor in the local actor registry.
    ///
    /// # Arguments
    ///
    /// * `ident` - The identifier to look up (name or UUID bytes)
    ///
    /// # Returns
    ///
    /// `Result<ActorRef<A>, LookupError>` - The actor reference or lookup error.
    ///
    /// # Errors
    ///
    /// Returns `LookupError` if:
    /// - Actor not found by the given identifier
    /// - Type mismatch between expected and actual actor type
    pub fn lookup_local<A: Actor>(
        &self,
        ident: impl AsRef<[u8]>,
    ) -> Result<ActorRef<A>, LookupError> {
        Self::lookup_local_impl(ident)
    }

    /// Bind an actor to a global identifier for lookup.
    ///
    /// # Arguments
    ///
    /// * `ident` - The identifier to bind the actor to
    /// * `actor` - The actor reference to bind
    pub fn bind<A: Actor>(&self, ident: impl Into<Ident>, actor: ActorRef<A>) {
        Self::bind_impl(ident.into(), actor);
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
        BINDINGS.write().unwrap().remove(ident.as_ref())
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

    pub(crate) fn lookup_local_impl<A: Actor>(
        ident: impl AsRef<[u8]>,
    ) -> Result<ActorRef<A>, LookupError> {
        let bindings = BINDINGS.read().unwrap();

        let actor = bindings.get(ident.as_ref()).ok_or(LookupError::NotFound)?;

        let actor = actor
            .as_any()
            .downcast_ref::<ActorRef<A>>()
            .ok_or(LookupError::TypeMismatch)?;

        Ok(actor.clone())
    }

    pub(crate) fn is_bound_impl<A: Actor>(ident: impl AsRef<[u8]>) -> bool {
        let bindings = BINDINGS.read().unwrap();
        let Some(actor) = bindings.get(ident.as_ref()) else {
            return false;
        };

        actor.as_any().is::<ActorRef<A>>()
    }

    pub(crate) fn bind_impl<A: Actor>(ident: Ident, actor: ActorRef<A>) {
        trace!(
            "Binding actor {} {} to {ident:?}",
            type_name::<A>(),
            actor.id()
        );
        BINDINGS.write().unwrap().insert(ident, Arc::new(actor));
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

    pub(crate) fn lookup_any_local_unchecked(
        ident: impl AsRef<[u8]>,
    ) -> Result<Arc<dyn AnyActorRef>, LookupError> {
        let bindings = BINDINGS.read().unwrap();

        let actor = bindings.get(ident.as_ref()).ok_or(LookupError::NotFound)?;

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
                        RawSignal::Escalation(e, escalation) => {
                            error!("Escalation received: {escalation:#?} for actor: {e:#?}");
                            e.raw_send(RawSignal::Terminate(None)).unwrap();
                        }
                        RawSignal::ChildDropped => {
                            debug!("A top-level actor has been dropped.");

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
    let _ = HDLS.write().unwrap().insert(actor_id, actor_hdl.clone());

    tokio::spawn({
        let actor = actor.downgrade();
        let parent_hdl = parent_hdl.clone();
        let actor_hdl = actor_hdl.clone();

        async move {
            let config = ActorConfig::new(actor, parent_hdl, actor_hdl, sig_rx, msg_rx, args);
            config.exec().await;

            #[cfg(feature = "monitor")]
            let _ = HDLS.write().unwrap().remove(&actor_id);
        }
    });

    (actor_hdl, actor)
}
