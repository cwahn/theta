use std::sync::{Arc, LazyLock, Mutex};

use futures::channel::oneshot;
use iroh::Endpoint;
use theta_flume::unbounded_with_id;
use tracing::{error, trace};
use uuid::Uuid;

#[cfg(feature = "monitor")]
use crate::monitor::HDLS;
use crate::{
    actor::{Actor, ActorArgs, ActorId},
    actor_instance::ActorConfig,
    actor_ref::{ActorHdl, ActorRef, AnyActorRef, WeakActorHdl, WeakActorRef},
    base::{BindingError, Hex, Ident, parse_ident},
    compat,
    message::RawSignal,
};
#[cfg(feature = "remote")]
use {
    crate::remote::{base::ActorTypeId, peer::LocalPeer},
    iroh::PublicKey,
};

/// Global registry mapping identifiers to actor references for named bindings.
/// Backed by ConcurrentMap for reduced contention on high lookup concurrency.
pub(crate) static BINDINGS: LazyLock<compat::ConcurrentMap<Ident, Arc<dyn AnyActorRef>>> =
    LazyLock::new(compat::ConcurrentMap::default);

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
#[derive(Debug)]
pub struct Context<A: Actor> {
    /// Weak reference to this actor instance
    pub this: WeakActorRef<A>,
    pub(crate) this_hdl: ActorHdl,
    pub(crate) child_hdls: Arc<Mutex<Vec<WeakActorHdl>>>,
}

impl<A: Actor> Clone for Context<A> {
    fn clone(&self) -> Self {
        Self {
            this: self.this.clone(),
            this_hdl: self.this_hdl.clone(),
            child_hdls: self.child_hdls.clone(),
        }
    }
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
///     ctx.bind("my_actor", actor);
/// }
/// ```
#[derive(Debug, Clone)]
pub struct RootContext {
    pub(crate) this_hdl: ActorHdl,
    pub(crate) child_hdls: Arc<Mutex<Vec<WeakActorHdl>>>,
}

impl<A: Actor> Context<A> {
    /// Get the unique identifier of this actor.
    ///
    /// # Return
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
    /// # Return
    ///
    /// `ActorRef<Args::Actor>` reference to the newly spawned actor.
    pub fn spawn<Args: ActorArgs>(&self, args: Args) -> ActorRef<Args::Actor> {
        let (hdl, actor) = spawn_impl(&self.this_hdl, args);

        self.child_hdls.lock().unwrap().push(hdl.downgrade());

        actor
    }

    /// Terminate this actor and all its children.
    pub async fn terminate(&self) {
        let (tx, rx) = oneshot::channel();

        self.this_hdl
            .raw_send(RawSignal::Terminate(Some(tx)))
            .unwrap();

        let _ = rx.await;
    }
}

impl RootContext {
    /// Initialize the root context with remote capabilities.
    ///
    /// # Arguments
    ///
    /// * `endpoint` - The iroh network endpoint for remote communication
    ///
    /// # Return
    ///
    /// `RootContext` with remote communication enabled.
    #[cfg(feature = "remote")]
    pub fn init(endpoint: Endpoint) -> Self {
        LocalPeer::init(endpoint);

        Self::init_local()
    }

    /// Initialize a local-only root context.
    ///
    /// # Return
    ///
    /// `RootContext` for local actor system operations.
    pub fn init_local() -> Self {
        Self::default()
    }

    /// Get the public key of this node for remote communication.
    ///
    /// # Return
    ///
    /// `PublicKey` for this node's identity in remote operations.
    #[cfg(feature = "remote")]
    pub fn public_key(&self) -> PublicKey {
        LocalPeer::inst().public_key()
    }

    /// Get a reference to the underlying iroh [`Endpoint`].
    ///
    /// This provides access to iroh's networking layer, useful for:
    /// - Checking connection type via [`Endpoint::remote_info`]
    /// - Monitoring path changes on connections
    /// - Retrieving the endpoint's address with [`Endpoint::addr`]
    ///
    /// [`Endpoint`]: iroh::Endpoint
    /// [`Endpoint::remote_info`]: iroh::Endpoint::remote_info
    /// [`Endpoint::addr`]: iroh::Endpoint::addr
    #[cfg(feature = "remote")]
    pub fn endpoint(&self) -> &Endpoint {
        LocalPeer::inst().endpoint()
    }

    /// Spawn a new top-level actor.
    ///
    /// # Arguments
    ///
    /// * `args` - The initialization arguments for the new actor
    ///
    /// # Return
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
    /// * `ident` - A name to bind the actor to (max 16 bytes)
    /// * `actor` - The actor reference to bind
    ///
    /// # Panics
    /// * If the identifier length exceeds 16 bytes
    pub fn bind<A: Actor>(
        &self,
        ident: impl AsRef<str>,
        actor: ActorRef<A>,
    ) -> Result<(), BindingError> {
        let ident = parse_ident(ident.as_ref())?;

        trace!(ident = % Hex(&ident), % actor, "binding");
        Self::bind_impl(ident, actor);

        Ok(())
    }

    /// Remove an actor binding from the global registry.
    ///
    /// # Arguments
    ///
    /// * `ident` - The identifier to remove from the registry
    ///
    /// # Return
    ///
    /// `Option<Arc<dyn AnyActorRef>>` - The removed binding if it existed.
    pub fn free(&self, ident: &[u8]) -> Option<Arc<dyn AnyActorRef>> {
        if ident.len() > 16 {
            return None;
        }

        let mut normalized: Ident = [0u8; 16];

        normalized[..ident.len()].copy_from_slice(ident);

        Self::free_impl(&normalized)
    }

    /// Terminate the root context and all spawned actors.
    ///
    /// Initiates system-wide shutdown and waits for all actors to terminate.
    ///
    /// # Note
    ///
    /// TODO: Make it support timeout for graceful shutdown with fallback to forced termination.
    pub async fn terminate(&self) {
        let (tx, rx) = oneshot::channel();

        self.this_hdl
            .raw_send(RawSignal::Terminate(Some(tx)))
            .unwrap();

        let _ = rx.await;
    }

    #[allow(dead_code)]
    pub(crate) fn is_bound_impl<A: Actor>(ident: &[u8; 16]) -> bool {
        BINDINGS
            .with(ident, |actor| {
                actor.as_any().is_some_and(|a| a.is::<ActorRef<A>>())
            })
            .unwrap_or(false)
    }

    pub(crate) fn bind_impl<A: AnyActorRef>(ident: Ident, actor: A) {
        let _ = BINDINGS.insert(ident, Arc::new(actor));
    }

    #[cfg(feature = "remote")]
    pub(crate) fn lookup_any_local_impl(
        actor_ty_id: ActorTypeId,
        ident: &[u8; 16],
    ) -> Result<Arc<dyn AnyActorRef>, BindingError> {
        match Self::lookup_any_local_unchecked_impl(ident) {
            Err(err) => Err(err),
            Ok(actor) => match actor.ty_id() == actor_ty_id {
                false => Err(BindingError::TypeMismatch),
                true => Ok(actor),
            },
        }
    }

    #[cfg(feature = "remote")]
    pub(crate) fn lookup_any_local_unchecked_impl(
        ident: &[u8; 16],
    ) -> Result<Arc<dyn AnyActorRef>, BindingError> {
        BINDINGS
            .with(ident, |actor| actor.clone())
            .ok_or(BindingError::NotFound)
    }

    pub(crate) fn free_impl(ident: &[u8; 16]) -> Option<Arc<dyn AnyActorRef>> {
        BINDINGS.remove(ident).map(|(_, v)| v)
    }
}

impl Default for RootContext {
    fn default() -> Self {
        let (sig_tx, sig_rx) = unbounded_with_id(Uuid::new_v4());
        let this_hdl = ActorHdl(sig_tx);
        let child_hdls = Arc::new(Mutex::new(Vec::<WeakActorHdl>::new()));

        compat::spawn({
            let child_hdls = child_hdls.clone();

            async move {
                while let Some(sig) = sig_rx.recv().await {
                    match sig {
                        RawSignal::Escalation(child_hdl, esc) => {
                            error!(child = % child_hdl.id(), ? esc, "root received escalation");

                            if let Err(e) = child_hdl.raw_send(RawSignal::Terminate(None)) {
                                error!(?e, "failed to send terminate to escalating child");
                            }
                        }
                        RawSignal::ChildDropped => {
                            let mut child_hdls = child_hdls.lock().unwrap();

                            child_hdls.retain(|hdl| match hdl.upgrade() {
                                None => false,
                                Some(hdl) => hdl.0.sender_count() > 0,
                            });
                        }
                        RawSignal::Terminate(k) => {
                            let alive_hdls: Vec<_> = child_hdls
                                .lock()
                                .unwrap()
                                .iter()
                                .filter_map(|hdl| hdl.upgrade())
                                .collect();
                            let child_rxs: Vec<_> = alive_hdls
                                .iter()
                                .filter_map(|hdl| {
                                    let (tx, rx) = oneshot::channel();

                                    match hdl.raw_send(RawSignal::Terminate(Some(tx))) {
                                        Ok(()) => Some(rx),
                                        Err(_) => None,
                                    }
                                })
                                .collect();

                            for rx in child_rxs {
                                let _ = rx.await;
                            }

                            if let Some(k) = k {
                                let _ = k.send(());
                            }
                        }
                        RawSignal::Pause(k) | RawSignal::Resume(k) | RawSignal::Restart(k) => {
                            if let Some(k) = k {
                                let _ = k.send(());
                            }
                        }
                        #[cfg(feature = "monitor")]
                        RawSignal::Monitor(_) => {}
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
    #[cfg(feature = "monitor")]
    let _ = HDLS.insert(actor_id, actor_hdl.clone());

    compat::spawn({
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
