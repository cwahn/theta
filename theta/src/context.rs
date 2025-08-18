use std::{
    any::type_name,
    sync::{Arc, LazyLock, Mutex, RwLock},
};

#[cfg(feature = "remote")]
use iroh::PublicKey;
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
    monitor::HDLS,
    remote::base::ActorTypeId,
    trace,
};

#[cfg(feature = "remote")]
use {
    crate::remote::{
        base::{RemoteError, split_url},
        peer::LocalPeer,
    },
    url::Url,
};

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
///     // Initialize local-only system
///     let ctx = RootContext::init_local();
///
///     // Spawn an actor
///     let actor = ctx.spawn(MyActor { value: 0 });
///
///     // Bind to a name for lookup
///     ctx.bind(b"my_actor", actor);
///     Ok(())
/// }
/// ```
#[derive(Debug, Clone)]
pub struct RootContext {
    pub(crate) this_hdl: ActorHdl,                        // Self reference
    pub(crate) child_hdls: Arc<Mutex<Vec<WeakActorHdl>>>, // children of the global context
}

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

#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum ObserveError {
    #[error(transparent)]
    LookupError(#[from] LookupError),
    #[error("failed to send signal")]
    SigSendError,
}

// Implementations

impl<A: Actor> Context<A> {
    /// Get the unique identifier of the actor that owns this context.
    ///
    /// # Returns
    ///
    /// The `ActorId` of the actor this context belongs to. This ID is unique
    /// within the actor system and remains constant for the actor's lifetime.
    /// Useful for logging, debugging, and actor identification.
    pub fn id(&self) -> ActorId {
        self.this_hdl.id()
    }

    /// Spawn a new child actor with the given arguments.
    ///
    /// Creates and starts a new actor as a child of the current actor.
    /// The child actor is automatically supervised by this actor and will
    /// be terminated when the parent terminates.
    ///
    /// # Arguments
    ///
    /// * `args` - The initialization arguments for the new actor. Must implement
    ///   `ActorArgs` and the associated `Actor` type defines the actor to spawn.
    ///
    /// # Returns
    ///
    /// An `ActorRef<Args::Actor>` that can be used to send messages to the
    /// newly spawned child actor.
    ///
    /// # Actor Hierarchy
    ///
    /// - Child actors are automatically supervised by their parent
    /// - Termination of parent will initiate termination of all children and parent will terminated after all children
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Spawn a child actor within an actor's message handler
    /// let child_ref = ctx.spawn(ChildActorArgs { config: config.clone() });
    /// child_ref.tell(InitializeChild)?;
    /// ```
    pub fn spawn<Args: ActorArgs>(&self, args: Args) -> ActorRef<Args::Actor> {
        let (hdl, actor) = spawn_impl(&self.this_hdl, args);

        self.child_hdls.lock().unwrap().push(hdl.downgrade());

        actor
    }

    /// Terminate this actor and all its children.
    ///
    /// Send Terminate signal to the actor.
    /// The signal will take effect immediately (right after processing of current message if any) and terminate all child actors before terminating itself.
    /// This operation is asynchronous and will wait for the termination to complete.
    ///
    /// # Termination Process
    ///
    /// 1. Actor stops accepting new messages
    /// 2. All child actors are terminated
    /// 3. Method returns when termination is complete
    ///
    /// # Usage
    ///
    /// Typically used to implement custom graceful shutdown logic by defining some message to do some cleanup tasks and invoke termination.
    ///
    /// # Cautions
    ///
    /// - **Non-termination**: Termination is just a message with high priority, it will not terminate hanging actors.
    /// - **Message loss**: Unprocessed messages in the mailbox will be discarded
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Terminate the actor in response to a shutdown message
    /// async |_: ShutdownMessage| {
    ///     info!("Received shutdown signal, terminating actor");
    ///     ctx.terminate().await;
    /// }
    /// ```
    ///
    /// # Note
    ///
    /// TODO: Make it support timeout for graceful shutdown with fallback.
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
    /// Sets up the actor system with networking support using the provided iroh endpoint.
    /// This enables communication with actors on remote nodes.
    ///
    /// # Arguments
    ///
    /// * `endpoint` - The iroh network endpoint for remote communication
    ///
    /// # Returns
    ///
    /// A `RootContext` configured for both local and remote actor operations.
    ///
    /// # Features
    ///
    /// This method is only available when the "remote" feature is enabled.
    #[cfg(feature = "remote")]
    pub fn init(endpoint: iroh::Endpoint) -> Self {
        LocalPeer::init(endpoint);

        Self::init_local()
    }

    /// Initialize a local-only root context.
    ///
    /// Creates a root context that can only manage local actors. This is suitable
    /// for single-node applications or when remote capabilities are not needed.
    ///
    /// # Returns
    ///
    /// A `RootContext` configured for local actor operations only.
    pub fn init_local() -> Self {
        Self::default()
    }

    /// Get the public key of this node for remote communication.
    ///
    /// # Returns
    ///
    /// The `PublicKey` that identifies this node in the distributed network.
    /// Other nodes can use this key to establish secure communication channels.
    ///
    /// # Features
    ///
    /// This method is only available when the "remote" feature is enabled.
    #[cfg(feature = "remote")]
    pub fn public_key(&self) -> PublicKey {
        LocalPeer::inst().public_key()
    }

    /// Spawn a new top-level actor.
    ///
    /// Creates and starts a new actor at the root level of the actor hierarchy.
    /// Root-level actors are not supervised by other actors and will run until
    /// explicitly terminated or the system shuts down.
    ///
    /// # Arguments
    ///
    /// * `args` - The initialization arguments for the new actor. Must implement
    ///   `ActorArgs` and the associated `Actor` type defines the actor to spawn.
    ///
    /// # Returns
    ///
    /// An `ActorRef<Args::Actor>` that can be used to send messages to the
    /// newly spawned actor.
    ///
    /// # Root Actor Characteristics
    ///
    /// - No parent supervision (only system-level supervision)
    /// - Persist until explicitly terminated
    /// - Suitable for service actors, managers, and entry points
    ///
    /// # Example
    ///
    /// ```ignore
    /// let root_ctx = RootContext::init_local();
    /// let service_actor = root_ctx.spawn(ServiceActorArgs::default());
    /// root_ctx.bind("main-service", service_actor.clone());
    /// ```
    pub fn spawn<Args: ActorArgs>(&self, args: Args) -> ActorRef<Args::Actor> {
        let (actor_hdl, actor) = spawn_impl(&self.this_hdl, args);

        self.child_hdls.lock().unwrap().push(actor_hdl.downgrade());

        actor
    }

    /// Look up an actor by identifier or URL.
    ///
    /// This method provides a unified interface for finding actors both locally and remotely.
    /// It automatically determines whether to perform a local or remote lookup based on the
    /// format of the provided identifier.
    ///
    /// # Arguments
    ///
    /// * `ident_or_url` - Either a local identifier or a full URL for remote actors.
    ///   - Local format: `"actor-name"` or `"actor-id"`
    ///   - Remote format: `"theta://public_key/actor-identifier"`
    ///
    /// # Returns
    ///
    /// - `Ok(ActorRef<A>)` if the actor is found and accessible
    /// - `Err(RemoteError)` if the lookup fails (actor not found, network error, etc.)
    ///
    /// # Network Behavior
    ///
    /// - Url format will invoke a remote lookup
    /// - Local format will perform a local lookup
    ///
    /// # Features
    ///
    /// This method is only available when the "remote" feature is enabled.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Look up a local actor
    /// let local_actor = root_ctx.lookup::<MyActor>("my-service").await?;
    ///
    /// // Look up a remote actor
    /// let remote_actor = root_ctx.lookup::<MyActor>(
    ///     "theta://abc123.../remote-service"
    /// ).await?;
    /// ```
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
    /// This method directly queries a remote node for an actor with the specified
    /// identifier and type. Unlike the general `lookup` method, this explicitly
    /// targets a known remote node by its public key.
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
    /// # Network Behavior
    ///
    /// - Establishes connection to the remote node if not already connected
    /// - Sends a lookup request and waits for response
    /// - May timeout if the remote node is unreachable
    /// - Response includes type verification to ensure actor compatibility
    ///
    ///
    /// # Features
    ///
    /// This method is only available when the "remote" feature is enabled.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let remote_key = PublicKey::from_str("abc123...")?;
    /// let result = root_ctx.lookup_remote::<DatabaseActor>(
    ///     "primary-db",
    ///     remote_key
    /// ).await?;
    ///
    /// match result {
    ///     Ok(db_actor) => {
    ///         // Found the remote database actor
    ///         db_actor.tell(QueryRequest(sql)).await?;
    ///     }
    ///     Err(LookupError::NotFound) => {
    ///         // Actor not found on remote node
    ///         println!("Database actor not available");
    ///     }
    /// }
    /// ```
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
    /// Searches for an actor with the specified identifier in the local node's
    /// actor registry. This is a synchronous operation that only searches
    /// locally bound actors.
    ///
    /// # Arguments
    ///
    /// * `ident` - The identifier to search for. Can be a string, byte slice,
    ///   or any type that can be converted to a byte reference.
    ///
    /// # Returns
    ///
    /// - `Ok(ActorRef<A>)` if a local actor with the identifier is found
    /// - `Err(LookupError)` if no actor is found or type mismatch occurs
    ///
    /// # Registry Behavior
    ///
    /// - Only searches actors bound with `bind()` method
    /// - Performs type checking to ensure the found actor matches type `A`
    /// - Returns immediately (no network delay)
    ///
    /// # Type Safety
    ///
    /// The method verifies that the found actor is of the expected type `A`.
    /// If an actor is found but has a different type, a `LookupError` is returned.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Look up a previously bound local actor
    /// match root_ctx.lookup_local::<ConfigService>("config") {
    ///     Ok(config_actor) => {
    ///         let config = config_actor.ask(GetConfig).await?;
    ///     }
    ///     Err(LookupError::NotFound) => {
    ///         println!("Config service not found");
    ///     }
    ///     Err(LookupError::TypeMismatch) => {
    ///         println!("Found actor but wrong type");
    ///     }
    /// }
    /// ```
    pub fn lookup_local<A: Actor>(
        &self,
        ident: impl AsRef<[u8]>,
    ) -> Result<ActorRef<A>, LookupError> {
        Self::lookup_local_impl(ident)
    }

    /// Bind an actor to a global identifier for lookup.
    ///
    /// Registers an actor in the global actor registry with the specified identifier.
    /// This makes the actor discoverable via `lookup_local()` and allows remote nodes
    /// to find it (if remote features are enabled).
    ///
    /// # Arguments
    ///
    /// * `ident` - The identifier to bind the actor to. Can be a string, byte slice,
    ///   or any type convertible to `Ident`. Should be unique within the system.
    /// * `actor` - The actor reference to bind to the identifier.
    ///
    /// # Registry Behavior
    ///
    /// - Overwrites any existing binding with the same identifier
    /// - Thread-safe registration in the global registry
    /// - The actor remains bound until explicitly freed or the system shuts down
    /// - Binding does not affect the actor's lifecycle (no additional supervision)
    ///
    /// # Example
    ///
    /// ```ignore
    /// let service_actor = root_ctx.spawn(DatabaseServiceArgs::default());
    ///
    /// // Bind the actor to a well-known identifier
    /// root_ctx.bind("database-service", service_actor);
    ///
    /// // Now other actors can find it
    /// let found_actor = root_ctx.lookup_local::<DatabaseService>("database-service")?;
    /// ```
    pub fn bind<A: Actor>(&self, ident: impl Into<Ident>, actor: ActorRef<A>) {
        Self::bind_impl(ident.into(), actor);
    }

    /// Remove an actor binding from the global registry.
    ///
    /// Removes the binding for the specified identifier from the global actor registry.
    /// This makes the actor no longer discoverable via lookup operations.
    ///
    /// # Arguments
    ///
    /// * `ident` - The identifier of the binding to remove.
    ///
    /// # Returns
    ///
    /// - `Some(Arc<dyn AnyActorRef>)` if a binding existed and was removed
    /// - `None` if no binding existed for the identifier
    ///
    /// # Behavior
    ///
    /// - Thread-safe removal from the global registry
    /// - Does not terminate the actor, only removes the binding
    /// - The actor continues to run but is no longer discoverable
    /// - Remote nodes will no longer be able to find the actor
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Remove a service binding during shutdown
    /// if let Some(removed_actor) = root_ctx.free("database-service") {
    ///     println!("Removed database service binding");
    ///     // The actor reference is returned but the actor keeps running
    /// } else {
    ///     println!("No binding found for database-service");
    /// }
    /// ```
    pub fn free(&self, ident: impl AsRef<[u8]>) -> Option<Arc<dyn AnyActorRef>> {
        BINDINGS.write().unwrap().remove(ident.as_ref())
    }

    /// Terminate the root context and all spawned actors.
    ///
    /// Initiates a system-wide shutdown by terminating the root context and all
    /// actors spawned from it. This is a graceful shutdown operation that waits
    /// for all actors to complete their cleanup.
    ///
    /// # Shutdown Process
    ///
    /// 1. Root context stops accepting new spawn requests
    /// 2. All child actors are signaled to terminate
    /// 3. Each actor performs its cleanup operations
    /// 4. Actor message channels are closed
    /// 5. Method returns when all actors have terminated
    ///
    /// # System Impact
    ///
    /// - **All actors terminate**: Every actor spawned from this root context
    /// - **Registry cleanup**: Actor bindings are automatically cleaned up
    /// - **Resource cleanup**: Network connections, files, etc. should be closed
    /// - **Graceful shutdown**: Actors have opportunity to save state, close resources
    ///
    /// # Cautions
    ///
    /// - **System-wide effect**: All actors in the system will terminate
    /// - **Blocking operation**: Method blocks until shutdown is complete
    /// - **No timeout**: Currently no timeout mechanism (see TODO below)
    /// - **Irreversible**: Once called, the system cannot be restarted
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Graceful application shutdown
    /// async fn shutdown_application(root_ctx: RootContext) {
    ///     info!("Initiating system shutdown...");
    ///     
    ///     // Terminate all actors gracefully
    ///     root_ctx.terminate().await;
    ///     
    ///     info!("All actors terminated successfully");
    /// }
    /// ```
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

pub(crate) fn spawn_impl<Args: ActorArgs>(
    parent_hdl: &ActorHdl,
    args: Args,
) -> (ActorHdl, ActorRef<Args::Actor>) {
    spawn_with_id_impl(Uuid::new_v4(), parent_hdl, args)
}

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
