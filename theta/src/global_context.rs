use std::{
    any::Any,
    borrow::Cow,
    collections::HashMap,
    str::FromStr,
    sync::{Arc, Mutex, RwLock},
};

use crate::{
    actor::{Actor, ActorConfig},
    actor_ref::{ActorHdl, WeakActorHdl},
    context::spawn_impl,
    message::{DynMessage, RawSignal},
    prelude::ActorRef,
};

#[cfg(feature = "remote")]
use crate::remote::peer::LocalPeer;
use anyhow::anyhow;
#[cfg(feature = "remote")]
use iroh::PublicKey;
use serde::{Deserialize, Serialize};
use tracing::{debug, error};
use url::Url;

pub(crate) type ActorBindings = Arc<RwLock<HashMap<Cow<'static, str>, Box<dyn Any + Send + Sync>>>>;

#[derive(Debug, Clone)]
pub struct GlobalContext {
    this_hdl: ActorHdl,
    child_hdls: Arc<Mutex<Vec<WeakActorHdl>>>,
    bindings: ActorBindings,

    local_peer: &'static LocalPeer,

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

        let bindings = ActorBindings::default();

        let local_peer = LocalPeer::initialize(bindings.clone()).await;

        Self {
            this_hdl,
            child_hdls,
            bindings,
            local_peer,
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

    #[cfg(feature = "remote")]
    pub async fn lookup<A: Actor>(&self, url: &Url) -> Option<ActorRef<A>>
    where
        DynMessage<A>: Serialize + for<'d> Deserialize<'d>,
    {
        if !self.is_iroh_url(url) {
            error!("Currently only iroh URLs are supported.");
            return None;
        }

        if self.is_host_local(url) {
            match self.parse_ident(url) {
                Ok(Some(local_ident)) => self.lookup_local::<A>(local_ident),
                Ok(None) => None,
                Err(e) => {
                    error!("Failed to parse local ident from URL: {e}");
                    None
                }
            }
        } else {
            match self.parse_iroh_pk_ident(url) {
                Ok(Some((public_key, ident))) => {
                    match self.local_peer.lookup::<A>(public_key, ident).await {
                        Ok(Some(actor)) => Some(actor),
                        Ok(None) => None,
                        Err(e) => {
                            error!("Failed to lookup actor: {e}");
                            None
                        }
                    }
                }
                Ok(None) => None,
                Err(e) => {
                    error!("Failed to parse iroh public key and ident from URL: {e}");
                    None
                }
            }
        }
    }

    /// Look up an actor by its ident in the current namespace.
    pub fn lookup_local<A: Actor>(&self, ident: impl AsRef<str>) -> Option<ActorRef<A>> {
        self.bindings
            .read()
            .unwrap()
            .get(ident.as_ref())
            .and_then(|actor| actor.downcast_ref::<ActorRef<A>>().cloned())
    }

    /// Bind an actor to a global name for later lookup.
    // This could not be any URL, but one in the current namespace.
    pub fn bind<A: Actor>(&self, ident: impl Into<Cow<'static, str>>, actor: ActorRef<A>) {
        self.bindings
            .write()
            .unwrap()
            .insert(ident.into(), Box::new(actor));
    }

    /// Remove an actor binding by its key.
    pub fn free(&self, ident: impl AsRef<str>) -> bool {
        self.bindings
            .write()
            .unwrap()
            .remove(ident.as_ref())
            .is_some()
    }

    #[cfg(feature = "remote")]
    pub fn public_key(&self) -> PublicKey {
        LocalPeer::get().public_key
    }

    // Helper methods

    fn is_iroh_url(&self, url: &Url) -> bool {
        url.scheme() == "iroh" && url.host_str().is_some()
    }

    fn is_host_local(&self, iroh_url: &Url) -> bool {
        iroh_url.host_str() == Some(&self.public_key().to_string())
    }

    fn parse_ident(&self, url: &Url) -> anyhow::Result<Option<String>> {
        // Check if the URL is in the format of "iroh://<public_key>/<ident>" and return the ident part.
        url.path_segments()
            .map(|mut segments| {
                let ident = match segments.next() {
                    Some(ident) => ident.to_string(),
                    None => return Err(anyhow!("Invalid URL format")),
                };

                if segments.next().is_none() {
                    Ok(ident)
                } else {
                    Err(anyhow!("Invalid URL format"))
                }
            })
            .transpose()
    }

    fn parse_iroh_pk_ident(&self, iroh_url: &Url) -> anyhow::Result<Option<(PublicKey, String)>> {
        let public_key = PublicKey::from_str(
            iroh_url
                .host_str()
                .ok_or_else(|| anyhow!("Invalid URL format: missing public key in host"))?,
        )?;

        let ident = self.parse_ident(iroh_url)?;

        Ok(ident.map(|i| (public_key, i)))
    }
}
