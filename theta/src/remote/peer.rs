use std::{
    collections::HashMap,
    sync::{Arc, Mutex, RwLock},
};

use futures::channel::oneshot;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use theta_flume::unbounded_with_id;
use theta_protocol::core::{Network, Transport};
use url::Url;
use uuid::Uuid;

use crate::{
    actor::Actor,
    base::{ActorImplId, Ident},
    binding::{AnyActorRef, BINDINGS},
    debug, error, errors,
    prelude::ActorRef,
    remote::{
        base::ReplyKey,
        serde::{CURRENT_PEER, LOCAL_PEER, MsgPackDto},
    },
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
    pending_recv_replies: Arc<Mutex<FxHashMap<ReplyKey, oneshot::Sender<Vec<u8>>>>>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Reply {
    reply_key: ReplyKey,
    reply_bytes: Vec<u8>,
}

impl LocalPeer {
    pub(crate) fn get_or_connect(&self, host_addr: &Url) -> Result<RemotePeer, RemoteError> {
        match self.0.remote_peers.read().unwrap().get(host_addr) {
            Some(peer) => Ok(peer.clone()),
            None => {
                let transport = self.0.network.lock().unwrap().connect(host_addr)?;
                let peer = RemotePeer::new(transport);

                self.0
                    .remote_peers
                    .write()
                    .unwrap()
                    .insert(host_addr.clone(), peer.clone());
                Ok(peer)
            }
        }
    }

    pub(crate) fn get_import<A: Actor>(&self, ident: &Ident) -> Option<Import<A>> {
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
    pub(crate) fn new(transport: Arc<dyn Transport>) -> Self {
        let this = Self {
            transport,
            pending_recv_replies: Arc::new(Mutex::new(FxHashMap::default())),
        };

        // Reply loop
        tokio::spawn({
            let this = this.clone();

            async move {
                loop {
                    let Ok(datagrame_bytes) = this.transport.recv_datagram().await else {
                        error!("Remote peer disconnected");
                        break;
                    };

                    // No need of context
                    let reply: Reply = match postcard::from_bytes(&datagrame_bytes) {
                        Ok(reply) => reply,
                        Err(e) => {
                            error!("Failed to deserialize reply: {e}");
                            continue;
                        }
                    };

                    let Some(reply_bytes_tx) = this
                        .pending_recv_replies
                        .lock()
                        .unwrap()
                        .remove(&reply.reply_key)
                    else {
                        error!("Reply key not found: {}", reply.reply_key);
                        continue;
                    };

                    if let Err(_) = reply_bytes_tx.send(reply.reply_bytes) {
                        warn!("Failed to send reply");
                    }
                }
            }
        });

        // Export loop
        tokio::spawn({
            let this = this.clone();

            async move {
                loop {
                    let Ok(in_stream) = this.transport.accept_uni().await else {
                        break error!("Failed to accept uni stream");
                    };

                    let Ok(lookup_bytes) = in_stream.recv_datagram().await else {
                        error!("Failed to receive lookup message");
                        continue;
                    };

                    let lookup: Lookup = match postcard::from_bytes(&lookup_bytes) {
                        Ok(lookup) => lookup,
                        Err(e) => {
                            error!("Failed to deserialize lookup message: {e}");
                            continue;
                        }
                    };

                    match BINDINGS.read().unwrap().get(&lookup.ident) {
                        Some(any_actor) => {
                            tokio::spawn(CURRENT_PEER.scope(this.clone(), async move {
                                // ! Can't get A at this point. Should be in registry
                                let Some(actor) = any_actor.downcast::<ActorRef<A>>() else {
                                    return error!("Lookup deserialization failed");
                                };

                                loop {
                                    let Ok(msg_k_bytes) = in_stream.recv_datagram().await else {
                                        break warn!("in_stream closed");
                                    };

                                    // ! Can't get A at this point. Should be in registry
                                    let (msg, k_dto): MsgPackDto<A> =
                                        match postcard::from_bytes(&msg_k_bytes) {
                                            Ok(x) => x,
                                            Err(e) => {
                                                break warn!("Failed to deserialize message: {e}");
                                            }
                                        };

                                    if let Err(e) = actor.send_raw(msg, k_dto.into()) {
                                        break error!("Failed to send message from remote: {e}");
                                    }
                                }
                            }));
                        }
                        None => {
                            error!("Actor with ident {:?} not found", lookup.ident);
                            continue;
                        }
                    }
                }
            }
        });

        this
    }

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
                    let Some((msg, k)) = msg_rx.recv().await else {
                        break debug!("Message channel closed, stopping remote actor");
                    };

                    // ! Should partially turn to Dto.
                    // Message could be serialized, and Continuation should be converted and serialized
                    let msg_k_bytes = match CURRENT_PEER.sync_scope(cloned_self.clone(), || {
                        let dto = MsgPackDto(msg, k.into());
                        postcard::to_stdvec(&dto)
                    }) {
                        Ok(bytes) => bytes,
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

    pub(crate) fn arrange_recv_reply(&self, reply_bytes_tx: oneshot::Sender<Vec<u8>>) -> ReplyKey {
        let reply_key = ReplyKey::new_v4();

        self.pending_recv_replies
            .lock()
            .unwrap()
            .insert(reply_key, reply_bytes_tx);

        reply_key
    }

    pub(crate) async fn send_reply(
        &self,
        reply_key: ReplyKey,
        reply_bytes: Vec<u8>,
    ) -> Result<(), RemoteError> {
        let reply = Reply {
            reply_key,
            reply_bytes,
        };

        if let Err(e) = self
            .transport
            .send_datagram(postcard::to_stdvec(&reply).unwrap()) // No need of context
            .await
        {
            error!("Failed to send reply: {e}");
            return Err(RemoteError::NetworkError(e));
        }

        Ok(())
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
