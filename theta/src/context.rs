use std::sync::{Arc, LazyLock, Mutex, RwLock};

use rustc_hash::FxHashMap;
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
    remote::peer::RemoteActorExt,
};

#[cfg(feature = "remote")]
use crate::{
    monitor::ReportTx,
    remote::base::{ExportTaskFn, RemoteError},
};

pub static BINDINGS: LazyLock<Bindings> = LazyLock::new(|| Bindings::default());

pub(crate) type Bindings = RwLock<FxHashMap<Ident, Binding>>;

#[derive(Debug, Clone)]
pub(crate) struct Binding {
    pub(crate) actor: Arc<dyn AnyActorRef>,
    #[cfg(feature = "remote")]
    pub(crate) export_task_fn: ExportTaskFn,
}

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

#[derive(Debug, Error)]
pub enum LookupError {
    #[error("actor not found")]
    NotFound,
    #[error("actor type mismatch")]
    TypeMismatch,
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
    pub async fn lookup<A: Actor>(&self, addr: Url) -> Result<Option<ActorRef<A>>, RemoteError> {
        todo!()
    }

    /// It is similar to [`import`] but wait until the response from the remote if the actor with the type and identity exists.
    #[cfg(feature = "remote")]
    pub async fn lookup_remote<A: Actor>(
        &self,
        host_addr: Url,
        ident: impl AsRef<[u8]>,
    ) -> Result<Option<ActorRef<A>>, RemoteError> {
        todo!()
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

    #[cfg(feature = "remote")]
    pub async fn observe<A: Actor>(&self, addr: Url, tx: ReportTx<A>) -> anyhow::Result<()> {
        todo!()
    }

    #[cfg(feature = "remote")]
    pub async fn observe_remote<A: Actor>(
        &self,
        host_addr: Url,
        ident: impl AsRef<[u8]>,
        tx: ReportTx<A>,
    ) -> anyhow::Result<()> {
        todo!()
    }

    pub fn observe_local<A: Actor>(&self, ident: impl AsRef<[u8]>, tx: ReportTx<A>) {
        todo!()
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
