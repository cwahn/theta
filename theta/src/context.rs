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
use uuid::Uuid;

use crate::{
    actor::{Actor, ActorArgs},
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

// Parent spawning child actor should not prevent another child from drop -> Should be WeakActorHdl
#[derive(Debug, Clone)]
pub struct Context<A: Actor> {
    pub this: WeakActorRef<A>,                            // Self reference
    pub(crate) this_hdl: ActorHdl,                        // Self supervision reference
    pub(crate) child_hdls: Arc<Mutex<Vec<WeakActorHdl>>>, // children of this actor
}

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
    pub fn spawn<Args: ActorArgs>(&self, args: Args) -> ActorRef<Args::Actor> {
        let (actor_hdl, actor) = spawn_impl(&self.this_hdl, args);

        self.child_hdls.lock().unwrap().push(actor_hdl.downgrade());

        actor
    }
}

impl RootContext {
    #[cfg(feature = "remote")]
    pub fn init(endpoint: iroh::Endpoint) -> Self {
        LocalPeer::init(endpoint);

        Self::init_local()
    }

    pub fn init_local() -> Self {
        Self::default()
    }

    #[cfg(feature = "remote")]
    pub fn public_key(&self) -> PublicKey {
        LocalPeer::inst().public_key()
    }

    pub fn spawn<Args: ActorArgs>(&self, args: Args) -> ActorRef<Args::Actor> {
        let (actor_hdl, actor) = spawn_impl(&self.this_hdl, args);

        self.child_hdls.lock().unwrap().push(actor_hdl.downgrade());

        actor
    }

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

    /// It is similar to [`import`] but wait until the response from the remote if the actor with the type and identity exists.
    #[cfg(feature = "remote")]
    pub async fn lookup_remote<A: Actor>(
        &self,
        ident: impl Into<Ident>,
        public_key: PublicKey,
    ) -> Result<Result<ActorRef<A>, LookupError>, RemoteError> {
        let peer = LocalPeer::inst().get_or_connect(public_key)?;

        peer.lookup(ident.into()).await
    }

    pub fn lookup_local<A: Actor>(
        &self,
        ident: impl AsRef<[u8]>,
    ) -> Result<ActorRef<A>, LookupError> {
        Self::lookup_local_impl(ident)
    }

    pub fn bind<A: Actor>(&self, ident: impl Into<Ident>, actor: ActorRef<A>) {
        Self::bind_impl(ident.into(), actor);
    }

    pub fn free(&self, ident: impl AsRef<[u8]>) -> Option<Arc<dyn AnyActorRef>> {
        BINDINGS.write().unwrap().remove(ident.as_ref())
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
    let id = Uuid::new_v4();
    let (msg_tx, msg_rx) = unbounded_with_id(id);
    let (sig_tx, sig_rx) = unbounded_with_id(id);

    let actor_hdl = ActorHdl(sig_tx);
    let actor = ActorRef(msg_tx);

    // Ignore chance of UUID v4 collision
    let _ = HDLS.write().unwrap().insert(id, actor_hdl.clone());

    tokio::spawn({
        let actor = actor.downgrade();
        let parent_hdl = parent_hdl.clone();
        let actor_hdl = actor_hdl.clone();

        async move {
            let config = ActorConfig::new(actor, parent_hdl, actor_hdl, sig_rx, msg_rx, args);
            config.exec().await;

            let _ = HDLS.write().unwrap().remove(&id);
        }
    });

    (actor_hdl, actor)
}
