use std::{
    any::Any,
    borrow::Cow,
    collections::HashMap,
    sync::{Arc, Mutex, RwLock},
};

use crate::{
    actor::{Actor, ActorConfig},
    actor_ref::{ActorHdl, WeakActorHdl},
    context::spawn_impl,
    message::RawSignal,
    prelude::ActorRef,
};

#[cfg(feature = "remote")]
use crate::remote::peer::LocalPeer;
#[cfg(feature = "remote")]
use iroh::PublicKey;
use tracing::{debug, error};

#[derive(Debug, Clone)]
pub struct GlobalContext {
    this_hdl: ActorHdl,
    child_hdls: Arc<Mutex<Vec<WeakActorHdl>>>,
    bindings: Arc<RwLock<HashMap<Cow<'static, str>, Box<dyn Any + Send + Sync>>>>,

    _task: Arc<Mutex<tokio::task::JoinHandle<()>>>,
}

impl GlobalContext {
    pub async fn initialize() -> Self {
        let (sig_tx, mut sig_rx) = tokio::sync::mpsc::unbounded_channel();
        let this_hdl = ActorHdl(sig_tx);
        let child_hdls = Arc::new(Mutex::new(Vec::<WeakActorHdl>::new()));

        let task = tokio::spawn({
            let child_hdls = child_hdls.clone();

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
                        _ => unreachable!(),
                    }
                }
            }
        });

        let _ = LocalPeer::initialize().await;

        Self {
            this_hdl,
            child_hdls,
            bindings: Arc::new(RwLock::new(HashMap::new())),
            _task: Arc::new(Mutex::new(task)),
        }
    }

    /// Spawn a new root actor with the given arguments.
    pub async fn spawn<C>(&self, args: C) -> ActorRef<C::Actor>
    where
        C: ActorConfig,
    {
        let (actor_hdl, actor) = spawn_impl(&self.this_hdl, args).await;
        self.child_hdls.lock().unwrap().push(actor_hdl.downgrade());
        actor
    }

    /// Bind an actor to a global name for later lookup.
    pub fn bind<A: Actor>(&self, key: impl Into<Cow<'static, str>>, actor: ActorRef<A>) {
        self.bindings
            .write()
            .unwrap()
            .insert(key.into(), Box::new(actor));
    }

    /// Look up an actor by its global name.
    pub fn lookup<A: Actor>(&self, key: &str) -> Option<ActorRef<A>> {
        self.bindings
            .read()
            .unwrap()
            .get(key)
            .and_then(|actor| actor.downcast_ref::<ActorRef<A>>().cloned())
    }

    /// Remove an actor binding by its key.
    pub fn free<Q>(&self, key: &Q) -> bool
    where
        Q: ?Sized + AsRef<str>,
    {
        self.bindings
            .write()
            .unwrap()
            .remove(key.as_ref())
            .is_some()
    }

    #[cfg(feature = "remote")]
    pub fn public_key(&self) -> PublicKey {
        LocalPeer::get().public_key
    }

    #[cfg(feature = "remote")]
    pub async fn connect_peer(&self, public_key: PublicKey) -> anyhow::Result<()> {
        LocalPeer::get().connect_peer(public_key).await?;

        Ok(())
    }
}
