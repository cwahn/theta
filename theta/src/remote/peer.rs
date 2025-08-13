use std::{
    any::type_name,
    collections::{HashMap, btree_map::Keys},
    sync::{Arc, Mutex, OnceLock, RwLock},
    time::Duration,
};

use futures::{channel::oneshot, future::BoxFuture};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use theta_flume::unbounded_with_id;
use theta_protocol::core::{Network, Receiver, Transport};
use tokio::task_local;
use url::Url;
use uuid::Uuid;

use crate::{
    actor::{self, Actor},
    actor_ref::AnyActorRef,
    base::Ident,
    context::{BINDINGS, LookupError, ObserveError, RootContext},
    debug, error,
    message::MsgPack,
    monitor::{AnyReportTx, Report, ReportTx},
    prelude::ActorRef,
    remote::{
        base::{ActorImplId, RemoteError, ReplyKey, Tag},
        serde::MsgPackDto,
    },
    warn,
};

pub(crate) static LOCAL_PEER: OnceLock<LocalPeer> = OnceLock::new();

task_local! {
    pub(crate) static CURRENT_PEER: Arc<RemotePeer>;
}

// ? Maybe use u64 instead
type LookupKey = Uuid;
type ObserveKey = Uuid;

pub(crate) trait RemoteActorExt: Actor {
    fn export_task_fn(
        remote_peer: Arc<RemotePeer>,
        in_stream: Box<dyn Receiver>,
        actor: Arc<dyn AnyActorRef>,
    ) -> BoxFuture<'static, ()> {
        Box::pin(CURRENT_PEER.scope(remote_peer, async move {
            let Some(actor) = actor.as_any().downcast_ref::<ActorRef<Self>>() else {
                return error!(
                    "Failed to downcast any actor reference to {}",
                    type_name::<Self>()
                );
            };

            loop {
                let Ok(bytes) = in_stream.recv_frame().await else {
                    break error!("Failed to receive frame from stream");
                };

                let Ok((msg, k_dto)) = postcard::from_bytes::<MsgPackDto<Self>>(&bytes) else {
                    warn!("Failed to deserialize msg pack dto");
                    continue;
                };

                let (msg, k): MsgPack<Self> = (msg, k_dto.into());

                if let Err(e) = actor.send_raw(msg, k) {
                    break error!("Failed to send message to actor: {e}");
                }
            }
        }))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct LocalPeer(Arc<LocalPeerInner>);

#[derive(Debug)]
pub(crate) struct RemotePeer {
    transport: Arc<dyn Transport>,
    state: RemotePeerState,
}

#[derive(Debug)]
struct LocalPeerInner {
    network: Mutex<Arc<dyn Network>>,
    remote_peers: RwLock<HashMap<Url, Arc<RemotePeer>>>, // ? Should I hold weak ref of remote peers?
    imports: RwLock<HashMap<Ident, AnyImport>>,
}

#[derive(Debug)]
struct RemotePeerState {
    pending_recv_replies: Mutex<FxHashMap<ReplyKey, oneshot::Sender<Vec<u8>>>>,
    pending_lookups: Mutex<FxHashMap<LookupKey, oneshot::Sender<Option<LookupError>>>>,
    pending_observe:
        Mutex<FxHashMap<ObserveKey, oneshot::Sender<Result<Box<dyn Receiver>, ObserveError>>>>,
}

#[derive(Debug)]
pub(crate) struct AnyImport {
    pub(crate) peer: Arc<RemotePeer>,
    pub(crate) ident: Ident, // Ident used for importing
    pub(crate) actor: Arc<dyn AnyActorRef>,
}

#[derive(Debug)]
pub(crate) struct Import<A: Actor> {
    pub(crate) peer: Arc<RemotePeer>,
    pub(crate) ident: Ident, // Ident used for importing
    pub(crate) actor: ActorRef<A>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum InitFrame {
    Import {
        ident: Ident,
    },
    Observe {
        mb_err: Option<ObserveError>,
        key: ObserveKey,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Datagram {
    Reply {
        reply_key: ReplyKey,
        reply_bytes: Vec<u8>,
    },
    Forward {
        ident: Ident,
        tag: Tag,
        bytes: Vec<u8>,
    },
    LookupReq {
        actor_impl_id: ActorImplId,
        ident: Ident,
        key: LookupKey,
    },
    LookupResp {
        mb_err: Option<LookupError>,
        key: LookupKey,
    },
    Observe {
        actor_impl_id: ActorImplId,
        ident: Ident,
        key: ObserveKey,
    },
}

// Implementations

impl<A: Actor> RemoteActorExt for A {}

impl LocalPeer {
    #[allow(dead_code)]
    pub fn init(network: Arc<dyn Network>) {
        let local_peer = LocalPeer(Arc::new(LocalPeerInner::new(network)));

        LOCAL_PEER
            .set(local_peer)
            .expect("Local peer should initialize only once");
    }

    pub(crate) fn inst() -> &'static LocalPeer {
        LOCAL_PEER.get().expect("Local peer is not initialized")
    }

    pub(crate) fn get_or_connect(&self, host_addr: &Url) -> Result<Arc<RemotePeer>, RemoteError> {
        match self.0.remote_peers.read().unwrap().get(host_addr) {
            Some(peer) => Ok(peer.clone()),
            None => {
                let transport = self.0.network.lock().unwrap().connect(host_addr)?;
                let peer = RemotePeer::new_arc(transport);

                self.0
                    .remote_peers
                    .write()
                    .unwrap()
                    .insert(host_addr.clone(), peer.clone());

                Ok(peer)
            }
        }
    }

    // ? Should I consider this as type of import?
    pub(crate) fn get_import<A: Actor>(&self, ident: &Ident) -> Option<Import<A>> {
        let imports = self.0.imports.read().unwrap();

        let import = imports.get(ident)?;
        let actor = import.actor.as_any().downcast_ref::<ActorRef<A>>()?;

        Some(Import {
            peer: import.peer.clone(),
            ident: Ident::from(import.ident.clone()),
            actor: actor.clone(),
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
    pub(crate) fn new_arc(transport: Arc<dyn Transport>) -> Arc<Self> {
        let this = Arc::new(Self {
            transport,
            state: RemotePeerState {
                pending_recv_replies: Mutex::new(FxHashMap::default()),
                pending_lookups: Mutex::new(FxHashMap::default()),
                pending_observe: Mutex::new(FxHashMap::default()),
            },
        });

        tokio::spawn({
            let this = this.clone();

            async move {
                loop {
                    let Ok(bytes) = this.transport.recv_frame().await else {
                        break error!("Remote peer disconnected");
                    };

                    // No need of context
                    let datagram: Datagram = match postcard::from_bytes(&bytes) {
                        Ok(reply) => reply,
                        Err(e) => {
                            error!("Failed to deserialize reply: {e}");
                            continue;
                        }
                    };

                    match datagram {
                        Datagram::Reply {
                            reply_key,
                            reply_bytes,
                        } => {
                            let Some(reply_bytes_tx) = this
                                .state
                                .pending_recv_replies
                                .lock()
                                .unwrap()
                                .remove(&reply_key)
                            else {
                                warn!("Reply key not found: {reply_key}");
                                continue;
                            };

                            if let Err(_) = reply_bytes_tx.send(reply_bytes) {
                                warn!("Failed to send reply");
                            }
                        }
                        Datagram::Forward { ident, tag, bytes } => {
                            let bindings = BINDINGS.read().unwrap();

                            let Some(binding) = bindings.get(&ident[..]) else {
                                warn!("Local actor reference not found in bindings");
                                continue;
                            };

                            if let Err(e) = binding.actor.send_tagged_bytes(tag, bytes) {
                                error!("Failed to send tagged bytes: {e}");
                                continue;
                            }
                        }
                        Datagram::LookupReq {
                            actor_impl_id,
                            ident,
                            key,
                        } => {
                            let mb_err = {
                                let bindings = BINDINGS.read().unwrap();

                                match bindings.get(&ident[..]) {
                                    Some(binding) if binding.actor.impl_id() == actor_impl_id => {
                                        None
                                    }
                                    Some(binding) => Some(LookupError::TypeMismatch),
                                    None => Some(LookupError::NotFound),
                                }
                            };

                            let resp = Datagram::LookupResp { mb_err, key: key };

                            let bytes = match postcard::to_stdvec(&resp) {
                                Ok(bytes) => bytes,
                                Err(e) => {
                                    error!("Failed to serialize lookup response: {e}");
                                    continue;
                                }
                            };

                            if let Err(e) = this.transport.send_frame(bytes).await {
                                error!("Failed to send lookup response: {e}");
                            }
                        }
                        Datagram::LookupResp { mb_err, key } => {
                            let Some(pending_lookup) =
                                this.state.pending_lookups.lock().unwrap().remove(&key)
                            else {
                                warn!("Pending lookup not found for key: {key}");
                                continue;
                            };

                            if let Err(_) = pending_lookup.send(mb_err) {
                                error!("Failed to send lookup response");
                            }
                        }
                        Datagram::Observe {
                            actor_impl_id,
                            ident,
                            key,
                        } => match RootContext::lookup_local_any(actor_impl_id, &ident) {
                            Err(e) => {
                                todo!()
                            }
                            Ok(any_actor) => {
                                let out_stream = self.transport.open_uni().await?;

                                any_actor.observe_as_bytes(peer, out_stream);
                                todo!("Need to factor out the handling functions");
                            }
                        },
                    }
                }
            }
        });

        // Stream handler loop
        // Incoming lookup & export
        // Observation
        tokio::spawn({
            let this = this.clone();

            async move {
                loop {
                    let Ok(in_stream) = this.transport.accept_uni().await else {
                        break error!("Failed to accept uni stream");
                    };

                    let Ok(lookup_bytes) = in_stream.recv_frame().await else {
                        error!("Failed to receive initial frame from stream");
                        continue;
                    };

                    // If I put to observe here, it needs opposite stream
                    // Actually it is more like exporting in sense direction of the stream.
                    // Export just make a binding and wait for the input.
                    // So it should be done in a similar manner.

                    let init_frame: InitFrame = match postcard::from_bytes(&lookup_bytes) {
                        Ok(lookup) => lookup,
                        Err(e) => {
                            error!("Failed to deserialize lookup message: {e}");
                            continue;
                        }
                    };

                    match BINDINGS.read().unwrap().get(&init_frame.ident) {
                        Some(binding) => {
                            tokio::spawn((binding.export_task_fn)(
                                this.clone(),
                                in_stream,
                                binding.actor.clone(),
                            ));
                        }
                        None => {
                            error!("Actor with ident {:?} not found", init_frame.ident);
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

            CURRENT_PEER.scope(self.clone(), async move {
                let Ok(out_stream) = cloned_self.transport.open_uni().await else {
                    return warn!("Failed to open uni stream");
                };

                let init_frame = InitFrame::Import { ident };

                let Ok(bytes) = postcard::to_stdvec(&init_frame) else {
                    return error!("Failed to serialize lookup message");
                };

                if let Err(e) = out_stream.send_frame(bytes).await {
                    return error!("Failed to send lookup message: {e}");
                }

                loop {
                    let Some((msg, k)) = msg_rx.recv().await else {
                        break debug!("Message channel closed, stopping remote actor");
                    };

                    let dto: MsgPackDto<A> = (msg, k.into_dto().await);

                    let msg_k_bytes = match postcard::to_stdvec(&dto) {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            break error!("Failed to convert message to DTO: {e}");
                        }
                    };

                    if let Err(e) = out_stream.send_frame(msg_k_bytes).await {
                        break error!("Failed to send message: {e}");
                    }
                }
            })
        });

        actor
    }

    pub(crate) async fn lookup<A: Actor>(&self, ident: Ident) -> Result<ActorRef<A>, RemoteError> {
        let resp = self.request_lookup(A::IMPL_ID, ident.clone()).await?;

        match resp {
            Some(e) => Err(e.into()),
            None => Ok(self.import(ident)),
        }
    }

    pub(crate) fn arrange_recv_reply(&self, reply_bytes_tx: oneshot::Sender<Vec<u8>>) -> ReplyKey {
        let reply_key = ReplyKey::new_v4();

        self.state
            .pending_recv_replies
            .lock()
            .unwrap()
            .insert(reply_key, reply_bytes_tx);

        reply_key
    }

    /// No need of task_local CURRENT_PEER
    pub(crate) async fn send_reply(
        &self,
        reply_key: ReplyKey,
        reply_bytes: Vec<u8>,
    ) -> Result<(), RemoteError> {
        self.send_datagram(Datagram::Reply {
            reply_key,
            reply_bytes,
        })
        .await
    }

    /// No need of task_local CURRENT_PEER
    pub(crate) async fn send_forward(
        &self,
        ident: Ident,
        tag: Tag,
        bytes: Vec<u8>,
    ) -> Result<(), RemoteError> {
        self.send_datagram(Datagram::Forward { ident, tag, bytes })
            .await
    }

    pub(crate) async fn observe<A: Actor>(
        &self,
        ident: Ident,
        tx: ReportTx<A>,
    ) -> Result<(), RemoteError> {
        let key = ObserveKey::new_v4();
        let (stream_tx, stream_rx) = oneshot::channel();
        let actor_impl_id = A::IMPL_ID;

        self.state
            .pending_observe
            .lock()
            .unwrap()
            .insert(key.clone(), stream_tx);

        self.send_datagram(Datagram::Observe {
            actor_impl_id,
            ident,
            key,
        })
        .await?;

        let in_stream = tokio::time::timeout(Duration::from_secs(5), stream_rx).await???;

        tokio::spawn({
            let cloned_self = self.clone();

            CURRENT_PEER.scope(cloned_self, async move {
                loop {
                    let Ok(bytes) = in_stream.recv_frame().await else {
                        break error!("Failed to receive frame");
                    };

                    let Ok(report) = postcard::from_bytes::<Report<A>>(&bytes) else {
                        warn!("Failed to deserialize report bytes");
                        continue;
                    };

                    if let Err(e) = tx.send(report) {
                        break warn!("Failed to send report: {e}");
                    }
                }
            })
        });

        Ok(())
    }

    pub(crate) async fn request_lookup(
        &self,
        actor_impl_id: ActorImplId,
        ident: Ident,
    ) -> Result<Option<LookupError>, RemoteError> {
        let key = Uuid::new_v4();
        let (tx, rx) = oneshot::channel();

        self.state
            .pending_lookups
            .lock()
            .unwrap()
            .insert(key.clone(), tx);

        self.send_datagram(Datagram::LookupReq {
            actor_impl_id,
            ident,
            key,
        })
        .await?;

        let resp = tokio::time::timeout(Duration::from_secs(5), rx).await??;

        Ok(resp)
    }

    /// No need of task_local CURRENT_PEER
    async fn send_datagram(&self, datagrame: Datagram) -> Result<(), RemoteError> {
        let bytes = postcard::to_stdvec(&datagrame).map_err(RemoteError::SerializeError)?;

        self.transport.send_frame(bytes).await?;

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
