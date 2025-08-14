use std::{
    any::type_name,
    collections::HashMap,
    sync::{
        Arc, Mutex, OnceLock, RwLock,
        atomic::{AtomicU64, Ordering},
    },
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
    actor::Actor,
    actor_ref::AnyActorRef,
    base::Ident,
    context::{LookupError, ObserveError, RootContext},
    debug, error,
    message::MsgPack,
    monitor::{Report, ReportTx},
    prelude::ActorRef,
    remote::{
        base::{ActorTypeId, RemoteError, ReplyKey, Tag},
        serde::MsgPackDto,
    },
    warn,
};

// ! Todo Network timeouts
// todo LocalPeer Drop guard

pub(crate) static LOCAL_PEER: OnceLock<LocalPeer> = OnceLock::new();

task_local! {
    pub(crate) static PEER: Peer;
}

// ? Maybe use u64 instead
type LookupKey = u64;
type ObserveKey = u64;

pub(crate) trait RemoteActorExt: Actor {
    fn export_task_fn(
        peer: Peer,
        in_stream: Box<dyn Receiver>,
        actor: Arc<dyn AnyActorRef>,
    ) -> BoxFuture<'static, ()> {
        Box::pin(PEER.scope(peer, async move {
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

#[derive(Debug, Clone)]
pub(crate) struct Peer(Arc<PeerInner>);

#[derive(Debug)]
pub(crate) struct Import<A: Actor> {
    pub(crate) peer: Peer,
    pub(crate) ident: Ident, // Ident used for importing
    pub(crate) actor: ActorRef<A>,
}

#[derive(Debug)]
struct LocalPeerInner {
    network: Mutex<Arc<dyn Network>>,
    peers: RwLock<HashMap<Url, Peer>>, // ? Should I hold weak ref of remote peers?
    imports: RwLock<HashMap<Ident, AnyImport>>,
}

#[derive(Debug)]
struct PeerInner {
    transport: Arc<dyn Transport>,
    state: PeerState,
}

#[derive(Debug)]
struct PeerState {
    next_key: AtomicU64,
    pending_recv_replies: Mutex<FxHashMap<ReplyKey, oneshot::Sender<Vec<u8>>>>,
    pending_lookups: Mutex<FxHashMap<LookupKey, oneshot::Sender<Option<LookupError>>>>,
    pending_observe:
        Mutex<FxHashMap<ObserveKey, oneshot::Sender<Result<Box<dyn Receiver>, ObserveError>>>>,
}

#[derive(Debug)]
struct AnyImport {
    pub(crate) peer: Peer,
    pub(crate) ident: Ident, // Ident used for importing
    pub(crate) actor: Arc<dyn AnyActorRef>,
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

// Only the lookup checks the actor_ty, after that consider it as checked.
// Locality suggest, even forward target is once exported-imported, which makes type check is not required.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Datagram {
    Reply {
        reply_key: ReplyKey,
        reply_bytes: Vec<u8>,
    },
    Forward {
        ident: Ident,
        tag: Tag, // todo Embed Tag to the bytes
        bytes: Vec<u8>,
    },
    LookupReq {
        actor_ty_id: ActorTypeId,
        ident: Ident,
        key: LookupKey,
    },
    LookupResp {
        mb_err: Option<LookupError>,
        key: LookupKey,
    },
    Observe {
        actor_ty_id: ActorTypeId,
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

    pub(crate) fn get_or_connect(&self, host_addr: &Url) -> Result<Peer, RemoteError> {
        match self.0.peers.read().unwrap().get(host_addr) {
            Some(peer) => Ok(peer.clone()),
            None => {
                let transport = self.0.network.lock().unwrap().connect(host_addr)?;
                let peer = Peer::new(transport);

                self.0
                    .peers
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
        let peer = self.get_or_connect(&host_addr)?;

        Ok(peer.import(ident))
    }
}

impl Peer {
    pub(crate) fn new(transport: Arc<dyn Transport>) -> Self {
        let this = Self(Arc::new(PeerInner {
            transport,
            state: PeerState {
                next_key: AtomicU64::default(),
                pending_recv_replies: Mutex::new(FxHashMap::default()),
                pending_lookups: Mutex::new(FxHashMap::default()),
                pending_observe: Mutex::new(FxHashMap::default()),
            },
        }));

        tokio::spawn({
            let this = this.clone();

            async move {
                loop {
                    let Ok(bytes) = this.0.transport.recv_frame().await else {
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
                        } => this.process_reply(reply_key, reply_bytes).await,
                        Datagram::Forward { ident, tag, bytes } => {
                            this.process_forward(ident, tag, bytes).await
                        }
                        Datagram::LookupReq {
                            actor_ty_id,
                            ident,
                            key,
                        } => this.process_lookup_req(actor_ty_id, ident, key).await,
                        Datagram::LookupResp { mb_err, key } => {
                            this.process_lookup_resp(mb_err, key).await
                        }
                        Datagram::Observe {
                            actor_ty_id,
                            ident,
                            key,
                        } => this.process_observe(actor_ty_id, ident, key).await,
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
                    let Ok(in_stream) = this.0.transport.accept_uni().await else {
                        break error!("Failed to accept uni stream");
                    };

                    let Ok(init_bytes) = in_stream.recv_frame().await else {
                        error!("Failed to receive initial frame from stream");
                        continue;
                    };

                    let init_frame: InitFrame = match postcard::from_bytes(&init_bytes) {
                        Ok(frame) => frame,
                        Err(e) => {
                            error!("Failed to deserialize lookup message: {e}");
                            continue;
                        }
                    };

                    match init_frame {
                        InitFrame::Import { ident } => {
                            let Ok(actor) = RootContext::lookup_any_local_unchecked(&ident) else {
                                error!("Local actor reference not found for ident: {ident:?}");
                                continue;
                            };

                            tokio::spawn((actor.export_task_fn_ptr())(
                                this.clone(),
                                in_stream,
                                actor.clone(),
                            ));
                        }
                        InitFrame::Observe { mb_err, key } => {
                            let Some(tx) =
                                this.0.state.pending_observe.lock().unwrap().remove(&key)
                            else {
                                warn!("Observation key not found: {key}");
                                continue;
                            };

                            let res = match mb_err {
                                None => Ok(in_stream),
                                Some(e) => Err(e),
                            };

                            if let Err(_) = tx.send(res) {
                                warn!("Failed to send observation stream result");
                            }
                        }
                    }
                }
            }
        });

        this
    }

    pub(crate) fn host_addr(&self) -> Url {
        self.0.transport.host_addr()
    }

    pub(crate) fn import<A: Actor>(&self, ident: Ident) -> ActorRef<A> {
        // Imported actor will have different id from the original actor
        let id = Uuid::new_v4();
        let (msg_tx, msg_rx) = unbounded_with_id(id);

        let actor = ActorRef(msg_tx);

        tokio::spawn({
            let cloned_self = self.clone();

            PEER.scope(self.clone(), async move {
                let Ok(out_stream) = cloned_self.0.transport.open_uni().await else {
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
        let reply_key = self.next_key();

        self.0
            .state
            .pending_recv_replies
            .lock()
            .unwrap()
            .insert(reply_key, reply_bytes_tx);

        reply_key
    }

    /// No need of task_local PEER
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

    /// No need of task_local PEER
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
        let key = self.next_key();
        let (stream_tx, stream_rx) = oneshot::channel();
        let actor_ty_id = A::IMPL_ID;

        self.0
            .state
            .pending_observe
            .lock()
            .unwrap()
            .insert(key.clone(), stream_tx);

        self.send_datagram(Datagram::Observe {
            actor_ty_id,
            ident,
            key,
        })
        .await?;

        let in_stream = tokio::time::timeout(Duration::from_secs(5), stream_rx).await???;

        tokio::spawn({
            let cloned_self = self.clone();

            PEER.scope(cloned_self, async move {
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
        actor_ty_id: ActorTypeId,
        ident: Ident,
    ) -> Result<Option<LookupError>, RemoteError> {
        let key = self.next_key();
        let (tx, rx) = oneshot::channel();

        self.0
            .state
            .pending_lookups
            .lock()
            .unwrap()
            .insert(key.clone(), tx);

        self.send_datagram(Datagram::LookupReq {
            actor_ty_id,
            ident,
            key,
        })
        .await?;

        let resp = tokio::time::timeout(Duration::from_secs(5), rx).await??;

        Ok(resp)
    }

    /// No need of task_local PEER
    async fn send_datagram(&self, datagrame: Datagram) -> Result<(), RemoteError> {
        let bytes = postcard::to_stdvec(&datagrame).map_err(RemoteError::SerializeError)?;

        self.0.transport.send_frame(bytes).await?;

        Ok(())
    }

    async fn process_reply(&self, reply_key: ReplyKey, reply_bytes: Vec<u8>) {
        let Some(reply_bytes_tx) = self
            .0
            .state
            .pending_recv_replies
            .lock()
            .unwrap()
            .remove(&reply_key)
        else {
            return warn!("Reply key not found: {reply_key}");
        };

        if let Err(_) = reply_bytes_tx.send(reply_bytes) {
            warn!("Failed to send reply");
        }
    }

    async fn process_forward(&self, ident: Ident, tag: Tag, bytes: Vec<u8>) {
        let actor = match RootContext::lookup_any_local_unchecked(&ident) {
            Ok(actor) => actor,
            Err(e) => {
                return warn!("Failed to lookup local actor: {e}");
            }
        };

        if let Err(e) = actor.send_tagged_bytes(tag, bytes) {
            error!("Failed to send tagged bytes: {e}");
        }
    }

    async fn process_lookup_req(&self, actor_ty_id: ActorTypeId, ident: Ident, key: ReplyKey) {
        let mb_err = RootContext::check_lookup_local(actor_ty_id, ident);

        let resp = Datagram::LookupResp { mb_err, key };

        let bytes = match postcard::to_stdvec(&resp) {
            Ok(bytes) => bytes,
            Err(e) => {
                return error!("Failed to serialize lookup response: {e}");
            }
        };

        if let Err(e) = self.0.transport.send_frame(bytes).await {
            error!("Failed to send lookup response: {e}");
        }
    }

    async fn process_lookup_resp(&self, mb_err: Option<LookupError>, key: ReplyKey) {
        let mut pending_lookups = self.0.state.pending_lookups.lock().unwrap();

        let Some(tx) = pending_lookups.remove(&key) else {
            return warn!("Lookup key not found: {key}");
        };

        if let Err(_) = tx.send(mb_err) {
            warn!("Failed to send lookup response");
        }
    }

    async fn process_observe(&self, actor_ty_id: ActorTypeId, ident: Ident, key: ReplyKey) {
        let actor = match RootContext::lookup_any_local(actor_ty_id, &ident) {
            Ok(actor) => actor,
            Err(e) => {
                return {
                    let init_frame = InitFrame::Observe {
                        mb_err: Some(e.into()),
                        key,
                    };

                    // ? Do I need PEER context? or already set
                    let init_bytes = match postcard::to_stdvec(&init_frame) {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            return error!("Failed to serialize init frame: {e}");
                        }
                    };

                    let out_stream = match self.0.transport.open_uni().await {
                        Ok(stream) => stream,
                        Err(e) => {
                            return error!("Failed to open uni stream for observation: {e}");
                        }
                    };

                    if let Err(e) = out_stream.send_frame(init_bytes).await {
                        return error!("Failed to send init frame: {e}");
                    }
                };
            }
        };

        let out_stream = match self.0.transport.open_uni().await {
            Ok(stream) => stream,
            Err(e) => {
                return error!("Failed to open uni stream for observation: {e}");
            }
        };

        if let Err(e) = actor.observe_as_bytes(self.clone(), out_stream) {
            return error!("Failed to observe actor as bytes: {e}");
        }
    }

    fn next_key(&self) -> ReplyKey {
        self.0.state.next_key.fetch_add(1, Ordering::Relaxed)
    }
}

impl LocalPeerInner {
    pub fn new(network: Arc<dyn Network>) -> Self {
        Self {
            network: Mutex::new(network),
            peers: RwLock::new(HashMap::new()),
            imports: RwLock::new(HashMap::new()),
        }
    }
}
