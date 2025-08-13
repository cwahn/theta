use std::sync::{Arc, LazyLock, Mutex, RwLock};

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use theta_flume::unbounded_with_id;
use thiserror::Error;
#[cfg(feature = "remote")]
use url::Url;
use uuid::Uuid;

use crate::{
    actor::{Actor, ActorArgs},
    actor_instance::ActorConfig,
    actor_ref::{ActorHdl, ActorRef, AnyActorRef, WeakActorHdl, WeakActorRef},
    base::Ident,
    debug, error,
    message::RawSignal,
    remote::{base::ActorImplId, peer::RemoteActorExt},
};

#[cfg(feature = "remote")]
use crate::remote::{
    base::{ExportTaskFn, RemoteError},
    peer::LocalPeer,
};

static BINDINGS: LazyLock<Bindings> = LazyLock::new(|| Bindings::default());

pub(crate) type Bindings = RwLock<FxHashMap<Ident, Binding>>;

#[derive(Debug, Clone)]
pub(crate) struct Binding {
    pub(crate) actor: Arc<dyn AnyActorRef>,
    // ! This should be method of AnyActorRef
    #[cfg(feature = "remote")]
    pub(crate) export_task_fn: ExportTaskFn,
}

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
        match Uuid::parse_str(ident_or_url.as_ref()) {
            Ok(ident) => Ok(self.lookup_local::<A>(&ident.as_bytes())?),
            Err(_) => {
                let url =
                    Url::parse(ident_or_url.as_ref()).map_err(|_| RemoteError::InvalidAddress)?;

                let (addr, ident) = Self::split_url(url)?;

                self.lookup_remote::<A>(addr, ident).await
            }
        }
    }

    /// It is similar to [`import`] but wait until the response from the remote if the actor with the type and identity exists.
    #[cfg(feature = "remote")]
    pub async fn lookup_remote<A: Actor>(
        &self,
        host_addr: Url,
        ident: impl Into<Ident>,
    ) -> Result<ActorRef<A>, RemoteError> {
        let remote_peer = LocalPeer::inst().get_or_connect(&host_addr)?;

        remote_peer.lookup(ident.into()).await
    }

    pub fn lookup_local<A: Actor>(
        &self,
        ident: impl AsRef<[u8]>,
    ) -> Result<ActorRef<A>, LookupError> {
        let bindings = BINDINGS.read().unwrap();

        let binding = bindings.get(ident.as_ref()).ok_or(LookupError::NotFound)?;

        let actor = binding
            .actor
            .as_any()
            .downcast_ref::<ActorRef<A>>()
            .ok_or(LookupError::TypeMismatch)?;

        Ok(actor.clone())
    }

    pub fn bind<A: Actor>(&self, ident: impl Into<Ident>, actor: ActorRef<A>) {
        BINDINGS.write().unwrap().insert(
            ident.into(),
            Binding {
                actor: Arc::new(actor),
                export_task_fn: <A as RemoteActorExt>::export_task_fn,
            },
        );
    }

    pub fn free(&self, ident: impl AsRef<[u8]>) -> Option<Arc<dyn AnyActorRef>> {
        BINDINGS
            .write()
            .unwrap()
            .remove(ident.as_ref())
            .map(|b| b.actor)
    }

    pub(crate) fn lookup_local_any(
        actor_impl_id: ActorImplId,
        ident: impl AsRef<[u8]>,
    ) -> Result<Arc<dyn AnyActorRef>, LookupError> {
        let bindings = BINDINGS.read().unwrap();

        let binding = bindings.get(ident.as_ref()).ok_or(LookupError::NotFound)?;

        if binding.actor.impl_id() != actor_impl_id {
            return Err(LookupError::TypeMismatch);
        }

        Ok(binding.actor.clone())
    }

    fn split_url(mut addr: Url) -> Result<(Url, Ident), RemoteError> {
        // read ident first
        let ident = addr
            .path_segments()
            .and_then(|it| it.filter(|s| !s.is_empty()).last())
            .ok_or(RemoteError::InvalidAddress)?
            .as_bytes()
            .to_vec()
            .into();

        // then drop it from the path

        {
            let mut segs = addr
                .path_segments_mut()
                .map_err(|_| RemoteError::InvalidAddress)?;

            segs.pop_if_empty();
            segs.pop(); // remove last real segment
        }

        Ok((addr, ident))
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
                            error!("Escalation received: {escalation:?} for actor: {e:?}");
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

    tokio::spawn({
        let actor = actor.downgrade();
        let parent_hdl = parent_hdl.clone();
        let actor_hdl = actor_hdl.clone();

        async move {
            let config = ActorConfig::new(actor, parent_hdl, actor_hdl, sig_rx, msg_rx, args);
            config.exec().await;
        }
    });

    (actor_hdl, actor)
}
