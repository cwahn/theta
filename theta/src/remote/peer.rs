use std::{
    collections::HashMap,
    sync::{Arc, Mutex, RwLock},
};

use serde::{Deserialize, Serialize};
use theta_flume::unbounded_with_id;
use theta_protocol::core::{Network, Transport};
// use tokio::sync::Mutex;
use url::Url;
use uuid::Uuid;

// #[cfg(feature = "tracing")]
// use tracing::{debug, error};

use crate::{
    actor::Actor,
    base::{ActorImplId, Ident},
    binding::AnyActorRef,
    debug, error,
    prelude::ActorRef,
    remote::serde::CURRENT_PEER,
    warn,
};

#[derive(Debug)]
pub enum RemoteError {
    InvalidAddress,
    NetworkError(theta_protocol::error::Error),
}

#[derive(Debug, Clone)]
pub(crate) struct LocalPeer(Arc<LocalPeerInner>);

#[derive(Debug, Clone)]
pub(crate) struct RemotePeer {
    transport: Arc<dyn Transport>,
}

#[derive(Debug)]
struct LocalPeerInner {
    network: Mutex<Arc<dyn Network>>,
    remote_peers: RwLock<HashMap<Url, RemotePeer>>, // ? Should I hold weak ref of remote peers?
    imports: RwLock<HashMap<Ident, AnyImport>>,
}

#[derive(Debug)]
pub(crate) struct AnyImport {
    pub(crate) peer: RemotePeer,
    pub(crate) ident: Vec<u8>, // Ident used for importing
    pub(crate) actor: AnyActorRef,
}

#[derive(Debug)]
pub(crate) struct Import<A: Actor> {
    pub(crate) peer: RemotePeer,
    pub(crate) ident: Ident, // Ident used for importing
    pub(crate) actor: ActorRef<A>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Lookup {
    actor_impl_id: ActorImplId,
    ident: Ident,
}

impl LocalPeer {
    pub(crate) fn get_or_connect(&self, host_addr: &Url) -> Result<RemotePeer, RemoteError> {
        match self.0.remote_peers.read().unwrap().get(host_addr) {
            Some(peer) => Ok(peer.clone()),
            None => {
                let transport = self.0.network.lock().unwrap().connect(host_addr)?;
                let peer = RemotePeer { transport };

                self.0
                    .remote_peers
                    .write()
                    .unwrap()
                    .insert(host_addr.clone(), peer.clone());
                Ok(peer)
            }
        }
    }

    pub(crate) fn get_imported<A: Actor>(&self, ident: &Ident) -> Option<Import<A>> {
        self.0
            .imports
            .read()
            .unwrap()
            .get(ident)
            .and_then(|any_import| {
                any_import
                    .actor
                    .downcast_ref::<ActorRef<A>>()
                    .cloned()
                    .map(|actor_ref| Import {
                        peer: any_import.peer.clone(),
                        ident: Ident::from(any_import.ident.clone()),
                        actor: actor_ref,
                    })
            })
    }

    /// Import will always spawn a new actor, but it may not connected to the remote peer successfully.
    pub(crate) fn import<A: Actor>(
        &self,
        host_addr: Url,
        ident: Ident,
    ) -> Result<ActorRef<A>, RemoteError> {
        let remote_peer = self.get_or_connect(&host_addr)?;

        Ok(remote_peer.import(ident))
    }
}

impl RemotePeer {
    pub(crate) fn host_addr(&self) -> Url {
        self.transport.host_addr()
    }

    pub(crate) fn import<A: Actor>(&self, ident: Ident) -> ActorRef<A> {
        // Imported actor will have different id from the original actor
        let id = Uuid::new_v4();
        let (msg_tx, msg_rx) = unbounded_with_id(id);

        let actor = ActorRef(msg_tx);

        tokio::spawn({
            let cloned_self = self.clone();

            async move {
                let Ok(out_stream) = cloned_self.transport.open_uni().await else {
                    return warn!("Failed to open uni stream");
                };

                let look_up = Lookup {
                    actor_impl_id: A::__IMPL_ID,
                    ident,
                };

                let Ok(init_datagram) = postcard::to_stdvec(&look_up) else {
                    return error!("Failed to serialize lookup message");
                };

                if let Err(e) = out_stream.send_datagram(init_datagram).await {
                    return error!("Failed to send lookup message: {e}");
                }

                loop {
                    let Some(msg_k) = msg_rx.recv().await else {
                        break debug!("Message channel closed, stopping remote actor");
                    };

                    let msg_k_bytes = match CURRENT_PEER
                        .sync_scope(cloned_self.clone(), || postcard::to_stdvec(&msg_k))
                    {
                        Ok(dto) => dto,
                        Err(e) => {
                            break error!("Failed to convert message to DTO: {e}");
                        }
                    };

                    if let Err(e) = out_stream.send_datagram(msg_k_bytes).await {
                        break error!("Failed to send message: {e}");
                    }
                }
            }
        });

        actor
    }
}

impl LocalPeerInner {
    pub fn new(network: Arc<dyn Network>) -> Self {
        Self {
            network: Mutex::new(network),
            remote_peers: RwLock::new(HashMap::new()),
            imports: RwLock::new(HashMap::new()),
        }
    }
}

impl From<theta_protocol::error::Error> for RemoteError {
    fn from(e: theta_protocol::error::Error) -> Self {
        RemoteError::NetworkError(e)
    }
}

impl core::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RemoteError::InvalidAddress => write!(f, "Invalid remote address"),
            RemoteError::NetworkError(e) => write!(f, "Network error: {}", e),
        }
    }
}

impl std::error::Error for RemoteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RemoteError::InvalidAddress => None,
            RemoteError::NetworkError(e) => Some(e),
        }
    }
}
