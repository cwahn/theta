use std::{
    collections::HashMap,
    fmt::Display,
    sync::{
        Arc, Mutex, OnceLock, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use futures::channel::oneshot;
use iroh::{NodeAddr, PublicKey};
use log::{debug, error, trace, warn};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use theta_flume::unbounded_with_id;
use tokio::task_local;

use crate::{
    actor::{Actor, ActorId},
    actor_ref::AnyActorRef,
    base::{Ident, MonitorError},
    context::{LookupError, RootContext},
    prelude::ActorRef,
    remote::{
        base::{ActorTypeId, RemoteError, ReplyKey, Tag},
        network::{Network, PreparedConn, RxStream, TxStream},
        serde::MsgPackDto,
    },
};

#[cfg(feature = "monitor")]
use crate::monitor::{HDLS, Update, UpdateTx};

// ! Todo Network timeouts
// todo LocalPeer Drop guard

/// Global singleton instance of the local peer for remote communication.
pub(crate) static LOCAL_PEER: OnceLock<LocalPeer> = OnceLock::new();

task_local! {
    pub(crate) static PEER: Peer;
}

// ? Maybe use u64 insteads
type LookupKey = u64;
type MonitorKey = u64;

/// Local peer handle for the current node's remote actor system.
#[derive(Debug, Clone)]
pub(crate) struct LocalPeer(Arc<LocalPeerInner>);

/// Handle to a remote peer in the distributed actor system.
#[derive(Debug, Clone)]
pub struct Peer(Arc<PeerInner>);

/// Imported remote actor reference with its originating peer.
#[derive(Debug)]
pub(crate) struct Import<A: Actor> {
    pub(crate) peer: Peer,
    pub(crate) actor: ActorRef<A>,
}

#[derive(Debug)]
struct LocalPeerInner {
    public_key: PublicKey,
    network: Network,
    peers: RwLock<HashMap<PublicKey, Peer>>, // ? Should I hold weak ref of remote peers?
    imports: RwLock<HashMap<ActorId, AnyImport>>,
}

type PendingRecvReplies = Mutex<FxHashMap<ReplyKey, oneshot::Sender<(Peer, Vec<u8>)>>>;
type PendingLookups = Mutex<FxHashMap<LookupKey, oneshot::Sender<Result<Vec<u8>, LookupError>>>>;

#[derive(Debug)]
struct PeerInner {
    public_key: PublicKey,
    // transport: Transport, // ! Should be Connection
    conn: PreparedConn,
    // contor_tx: TxStream,
    // contor_rx: RxStream,
    state: PeerState,
}

#[derive(Debug)]
struct PeerState {
    next_key: AtomicU64,
    pending_recv_replies: PendingRecvReplies,
    pending_lookups: PendingLookups,
    pending_monitors: Mutex<FxHashMap<MonitorKey, oneshot::Sender<Result<RxStream, MonitorError>>>>,
}

#[derive(Debug)]
struct AnyImport {
    pub(crate) peer: Peer,
    pub(crate) actor: Arc<dyn AnyActorRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum InitFrame {
    Import {
        actor_id: ActorId,
    },
    Monitor {
        mb_err: Option<MonitorError>,
        key: MonitorKey,
    },
}

// Only the lookup checks the actor_ty, after that consider it as checked.
// Locality suggest, even forward target is once exported-imported, which makes type check is not required.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum ControlFrame {
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
        res: Result<Vec<u8>, LookupError>,
        key: LookupKey,
    },
    Monitor {
        actor_ty_id: ActorTypeId,
        ident: Ident,
        key: MonitorKey,
    },
    // Remote monitoring needs AnyActorRef, which means no need of MonitorId that does not have type information
}

// Implementations

impl LocalPeer {
    #[allow(dead_code)]
    pub fn init(endpoint: iroh::Endpoint) {
        let this = LocalPeer(Arc::new(LocalPeerInner::new(Network::new(endpoint))));

        tokio::spawn({
            let this = this.clone();

            async move {
                loop {
                    let (public_key, conn) = match this.0.network.accept_and_prepare().await {
                        Err(e) => {
                            error!("Failed to accept conn: {e}");
                            continue;
                        }
                        Ok(x) => x,
                    };
                    debug!("Incoming connection from {public_key}");

                    let peer = Peer::new(public_key, conn);
                    this.add_peer(public_key, peer);
                }
            }
        });

        LOCAL_PEER
            .set(this)
            .expect("Local peer should initialize only once");
    }

    pub(crate) fn inst() -> &'static LocalPeer {
        LOCAL_PEER.get().expect("Local peer is not initialized")
    }

    pub(crate) fn public_key(&self) -> PublicKey {
        self.0.public_key
    }

    pub(crate) fn get_or_connect(&self, public_key: PublicKey) -> Result<Peer, RemoteError> {
        if let Some(peer) = self.get_peer(&public_key) {
            trace!("Found existing connection for {public_key}");
            return Ok(peer);
        }

        // let transport = self.0.network.connect(NodeAddr::new(public_key));
        let conn = self
            .0
            .network
            .connect_and_prepare(NodeAddr::new(public_key));

        let peer = Peer::new(public_key, conn);

        self.add_peer(public_key, peer.clone());

        Ok(peer)
    }

    pub(crate) fn get_import<A: Actor>(&self, actor_id: ActorId) -> Option<Import<A>> {
        let imports = self.0.imports.read().unwrap();
        let import = imports.get(&actor_id)?;

        let actor = import.actor.as_any().downcast_ref::<ActorRef<A>>()?;

        Some(Import {
            peer: import.peer.clone(),
            actor: actor.clone(),
        })
    }

    /// Import will always spawn a new actor, but it may not connected to the remote peer successfully.
    pub(crate) fn import<A: Actor>(
        &self,
        actor_id: ActorId,
        public_key: PublicKey,
    ) -> Result<ActorRef<A>, RemoteError> {
        let peer = self.get_or_connect(public_key)?;

        Ok(peer.import(actor_id))
    }

    fn add_peer(&self, public_key: PublicKey, peer: Peer) {
        trace!("Adding peer {public_key} to the local peers");
        if let Some(old_peer) = self.0.peers.write().unwrap().insert(public_key, peer) {
            warn!(
                "Replacing existing peer connection for {}",
                old_peer.public_key()
            );
        }
        debug!("Peer {public_key} added to the local peers");
    }

    fn get_peer(&self, public_key: &PublicKey) -> Option<Peer> {
        self.0.peers.read().unwrap().get(public_key).cloned()
    }

    fn remove_peer(&self, public_key: &PublicKey) {
        trace!("Removing peer {public_key} from the local peers");
        if self.0.peers.write().unwrap().remove(public_key).is_none() {
            warn!("Peer {public_key} not found in the local peers");
        }
        debug!("Peer {public_key} removed from the local peers");
    }
}

impl Peer {
    pub(crate) fn new(public_key: PublicKey, conn: PreparedConn) -> Self {
        let this = Self(Arc::new(PeerInner {
            public_key,
            conn,
            state: PeerState {
                next_key: AtomicU64::default(),
                pending_recv_replies: Mutex::new(FxHashMap::default()),
                pending_lookups: Mutex::new(FxHashMap::default()),
                pending_monitors: Mutex::new(FxHashMap::default()),
            },
        }));

        tokio::spawn({
            let this = this.clone();

            async move {
                trace!("Peer connection loop started for {}", this.0.public_key);

                let mut control_rx = match this.0.conn.control_rx().await {
                    Err(e) => {
                        error!("Failed to get control_rx: {e}");
                        return LocalPeer::inst().remove_peer(&this.0.public_key);
                    }
                    Ok(rx) => rx,
                };

                // Incoming lookup & export handling loop
                tokio::spawn({
                    let this = this.clone();

                    async move {
                        trace!("Peer stream handler loop started for {}", this.0.public_key);

                        loop {
                            let mut in_stream = match this.0.conn.accept_uni().await {
                                Err(e) => break error!("Failed to accept uni stream: {e}"),
                                Ok(s) => s,
                            };
                            debug!("Accepted uni stream from {}", this.0.public_key);

                            let init_bytes = match in_stream.recv_frame().await {
                                Err(e) => {
                                    break error!(
                                        "Failed to receive initial frame from stream: {e}"
                                    );
                                }
                                Ok(bytes) => bytes,
                            };
                            debug!("Received initial frame from {}", this.0.public_key);

                            let init_frame: InitFrame = match postcard::from_bytes(&init_bytes) {
                                Err(e) => {
                                    error!("Failed to deserialize lookup message: {e}");
                                    continue;
                                }
                                Ok(frame) => frame,
                            };

                            match init_frame {
                                InitFrame::Import { actor_id } => {
                                    let Ok(actor) =
                                        RootContext::lookup_any_local_unchecked(actor_id)
                                    else {
                                        error!(
                                            "Local actor reference not found for ident: {actor_id}"
                                        );
                                        continue;
                                    };

                                    trace!(
                                        "Spawning export listener task for actor {}",
                                        actor.id()
                                    );
                                    tokio::spawn((actor.export_task_fn())(
                                        this.clone(),
                                        in_stream,
                                        actor.clone(),
                                    ));
                                }
                                InitFrame::Monitor { mb_err, key } => {
                                    let Some(tx) =
                                        this.0.state.pending_monitors.lock().unwrap().remove(&key)
                                    else {
                                        warn!("Monitoring key not found: {key}");
                                        continue;
                                    };

                                    let res = match mb_err {
                                        Some(e) => Err(e),
                                        None => Ok(in_stream),
                                    };

                                    if tx.send(res).is_err() {
                                        warn!("Failed to send monitoring stream result");
                                    }
                                }
                            }
                        }

                        LocalPeer::inst().remove_peer(&this.0.public_key);
                    }
                });

                loop {
                    let bytes = match control_rx.recv_frame().await {
                        Err(e) => {
                            break error!("Failed to receive control frame: {e}");
                        }
                        Ok(bytes) => bytes,
                    };

                    debug!(
                        "Received control frame {} bytes from {}",
                        bytes.len(),
                        this.0.public_key
                    );

                    // No need of context
                    let frame: ControlFrame = match postcard::from_bytes(&bytes) {
                        Err(e) => {
                            error!("Failed to deserialize frame: {e}");
                            continue;
                        }
                        Ok(frame) => frame,
                    };
                    debug!("Deserialized control frame: {frame}");

                    match frame {
                        ControlFrame::Reply {
                            reply_key,
                            reply_bytes,
                        } => this.process_reply(reply_key, reply_bytes).await,
                        ControlFrame::Forward { ident, tag, bytes } => {
                            this.process_forward(ident, tag, bytes).await
                        }
                        ControlFrame::LookupReq {
                            actor_ty_id,
                            ident,
                            key,
                        } => this.process_lookup_req(actor_ty_id, ident, key).await,
                        ControlFrame::LookupResp { res, key } => {
                            this.process_lookup_resp(res, key).await
                        }
                        ControlFrame::Monitor {
                            actor_ty_id,
                            ident,
                            key,
                        } => this.process_monitor(actor_ty_id, ident, key).await,
                    }
                }

                LocalPeer::inst().remove_peer(&this.0.public_key);
            }
        });

        this
    }

    pub(crate) fn public_key(&self) -> PublicKey {
        self.0.public_key
    }

    pub(crate) fn import<A: Actor>(&self, actor_id: ActorId) -> ActorRef<A> {
        let (msg_tx, msg_rx) = unbounded_with_id(actor_id);

        let actor = ActorRef(msg_tx);

        tokio::spawn({
            let cloned_self = self.clone();

            PEER.scope(self.clone(), async move {
                let mut out_stream = match cloned_self.0.conn.open_uni().await {
                    Err(e) => return warn!("Failed to open uni stream: {e}"),
                    Ok(stream) => stream,
                };

                trace!("Sending import init frame for ident: {actor_id:?}");
                let init_frame = InitFrame::Import { actor_id };

                let bytes = match postcard::to_stdvec(&init_frame) {
                    Err(e) => return error!("Failed to serialize lookup message: {e}"),
                    Ok(bytes) => bytes,
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
                        Err(e) => break error!("Failed to convert message to DTO: {e}"),
                        Ok(bytes) => bytes,
                    };

                    trace!(
                        "Sending message pack to remote: {} bytes",
                        msg_k_bytes.len()
                    );
                    if let Err(e) = out_stream.send_frame(msg_k_bytes).await {
                        break error!("Failed to send message: {e}");
                    }
                }
            })
        });

        LocalPeer::inst().0.imports.write().unwrap().insert(
            actor.id(),
            AnyImport {
                peer: self.clone(),
                actor: Arc::new(actor.clone()),
            },
        );

        actor
    }

    pub(crate) fn arrange_recv_reply(
        &self,
        reply_bytes_tx: oneshot::Sender<(Peer, Vec<u8>)>,
    ) -> ReplyKey {
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
        self.send_control_frame(ControlFrame::Reply {
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
        self.send_control_frame(ControlFrame::Forward { ident, tag, bytes })
            .await
    }

    #[cfg(feature = "monitor")]
    pub(crate) async fn monitor<A: Actor>(
        &self,
        ident: Ident,
        tx: UpdateTx<A>,
    ) -> Result<(), RemoteError> {
        let key = self.next_key();
        let (stream_tx, stream_rx) = oneshot::channel();

        self.0
            .state
            .pending_monitors
            .lock()
            .unwrap()
            .insert(key, stream_tx);

        let frame = ControlFrame::Monitor {
            actor_ty_id: A::IMPL_ID,
            ident,
            key,
        };

        trace!("Sending monitor request control frame {frame:?}, key: {key}");
        self.send_control_frame(frame).await?;

        let mut in_stream = tokio::time::timeout(Duration::from_secs(5), stream_rx).await???;

        tokio::spawn({
            let this = self.clone();

            PEER.scope(this, async move {
                loop {
                    let Ok(bytes) = in_stream.recv_frame().await else {
                        break error!("Failed to receive frame");
                    };

                    let Ok(update) = postcard::from_bytes::<Update<A>>(&bytes) else {
                        warn!("Failed to deserialize update bytes");
                        continue;
                    };

                    if let Err(e) = tx.send(update) {
                        break warn!("Failed to send update: {e}");
                    }
                }
            })
        });

        Ok(())
    }

    pub(crate) async fn lookup<A: Actor>(
        &self,
        ident: Ident,
    ) -> Result<Result<ActorRef<A>, LookupError>, RemoteError> {
        let key = self.next_key();
        let (tx, rx) = oneshot::channel();

        self.0.state.pending_lookups.lock().unwrap().insert(key, tx);

        trace!("Sending lookup request for ident: {ident:02x?}, key: {key}");
        self.send_control_frame(ControlFrame::LookupReq {
            actor_ty_id: A::IMPL_ID,
            ident,
            key,
        })
        .await?;

        let resp = tokio::time::timeout(Duration::from_secs(5), rx).await??;
        debug!("Received lookup response for key: {key}, response: {resp:?}");

        let bytes = match resp {
            Err(e) => return Ok(Err(e)),
            Ok(bytes) => bytes,
        };

        let actor = PEER
            .sync_scope(self.clone(), || postcard::from_bytes::<ActorRef<A>>(&bytes))
            .map_err(RemoteError::DeserializeError)?;

        Ok(Ok(actor))
    }

    /// No need of task_local PEER
    async fn send_control_frame(&self, frame: ControlFrame) -> Result<(), RemoteError> {
        let bytes = postcard::to_stdvec(&frame).map_err(RemoteError::SerializeError)?;

        self.0.conn.send_frame(bytes).await?;

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

        if reply_bytes_tx.send((self.clone(), reply_bytes)).is_err() {
            warn!("Failed to send reply");
        }
    }

    async fn process_forward(&self, ident: Ident, tag: Tag, bytes: Vec<u8>) {
        let actor = match RootContext::lookup_any_local_unchecked(&ident) {
            Err(e) => return warn!("Failed to lookup local actor: {e}"),
            Ok(actor) => actor,
        };

        if let Err(e) = actor.send_tagged_bytes(tag, bytes) {
            error!("Failed to send tagged bytes: {e}");
        }
    }

    async fn process_lookup_req(&self, actor_ty_id: ActorTypeId, ident: Ident, key: ReplyKey) {
        trace!("Processing lookup request for ident: {ident:02x?}, key: {key}");
        let res = RootContext::lookup_any_local(actor_ty_id, &ident);

        let resp = match res {
            Err(e) => ControlFrame::LookupResp { res: Err(e), key },
            Ok(any_actor) => match PEER.sync_scope(self.clone(), || any_actor.serialize()) {
                Err(e) => ControlFrame::LookupResp { res: Err(e), key },
                Ok(actor) => ControlFrame::LookupResp {
                    res: Ok(actor),
                    key,
                },
            },
        };
        debug!("Processed lookup request key: {key:?}, with response: {resp:?}");

        let bytes = match postcard::to_stdvec(&resp) {
            Err(e) => return error!("Failed to serialize lookup response: {e}"),
            Ok(b) => b,
        };

        trace!("Sending lookup response for key: {key}");
        if let Err(e) = self.0.conn.send_frame(bytes).await {
            error!("Failed to send lookup response: {e}");
        }
    }

    async fn process_lookup_resp(&self, lookup_res: Result<Vec<u8>, LookupError>, key: ReplyKey) {
        let mut pending_lookups = self.0.state.pending_lookups.lock().unwrap();

        let Some(tx) = pending_lookups.remove(&key) else {
            return warn!("Lookup key not found: {key}");
        };

        if tx.send(lookup_res).is_err() {
            warn!("Failed to send lookup response");
        }
    }

    async fn process_monitor(&self, actor_ty_id: ActorTypeId, ident: Ident, key: ReplyKey) {
        trace!("Processing monitoring request for ident: {ident:02x?}, key: {key}");
        let _actor = match RootContext::lookup_any_local(actor_ty_id, &ident) {
            Err(e) => {
                return {
                    trace!("Failed to lookup actor {ident:02x?} for remote monitoring: {e}");
                    let init_frame = InitFrame::Monitor {
                        mb_err: Some(e.into()),
                        key,
                    };

                    if let Err(e) = self.open_uni_with(init_frame).await {
                        error!("Failed to open uni stream for monitoring: {e}");
                    }
                };
            }
            Ok(actor) => actor,
        };

        #[cfg(feature = "monitor")]
        {
            let hdl = {
                let mb_hdl = HDLS.read().unwrap().get(&_actor.id()).cloned();

                match mb_hdl {
                    None => {
                        let init_frame = InitFrame::Monitor {
                            mb_err: Some(LookupError::NotFound.into()),
                            key,
                        };

                        if let Err(e) = self.open_uni_with(init_frame).await {
                            error!("Failed to open uni stream for monitoring: {e}");
                        }

                        return;
                    }
                    Some(h) => h,
                }
            };

            let init_frame = InitFrame::Monitor { mb_err: None, key };

            let out_stream = match self.open_uni_with(init_frame).await {
                Err(e) => return error!("Failed to open uni stream for monitoring: {e}"),
                Ok(out_stream) => out_stream,
            };

            if let Err(e) = _actor.monitor_as_bytes(self.clone(), hdl, out_stream) {
                error!("Failed to monitor actor as bytes: {e}")
            }
        }
    }

    async fn open_uni_with(&self, init_frame: InitFrame) -> Result<TxStream, RemoteError> {
        let mut out_stream = self.0.conn.open_uni().await?;

        let init_bytes = postcard::to_stdvec(&init_frame).map_err(RemoteError::SerializeError)?;

        out_stream.send_frame(init_bytes).await?;

        Ok(out_stream)
    }

    fn next_key(&self) -> ReplyKey {
        self.0.state.next_key.fetch_add(1, Ordering::Relaxed)
    }
}

impl LocalPeerInner {
    pub fn new(network: Network) -> Self {
        Self {
            public_key: network.public_key(),
            network,
            peers: RwLock::new(HashMap::new()),
            imports: RwLock::new(HashMap::new()),
        }
    }
}

impl Display for ControlFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlFrame::Reply {
                reply_key,
                reply_bytes,
            } => {
                write!(
                    f,
                    "ControlFrame::Reply {{ reply_key: {reply_key}, reply_bytes: {} bytes }}",
                    reply_bytes.len()
                )
            }
            ControlFrame::Forward { ident, tag, bytes } => write!(
                f,
                "ControlFrame::Forward {{ ident: {ident:02x?}, tag: {tag:?}, bytes: {} bytes }}",
                bytes.len()
            ),
            ControlFrame::LookupReq {
                actor_ty_id,
                ident,
                key,
            } => write!(
                f,
                "ControlFrame::LookupReq {{ actor_ty_id: {actor_ty_id}, ident: {ident:02x?}, key: {key} }}"
            ),
            ControlFrame::LookupResp { res, key } => {
                write!(f, "ControlFrame::LookupResp {{ res: {res:?}, key: {key} }}")
            }
            ControlFrame::Monitor {
                actor_ty_id,
                ident,
                key,
            } => write!(
                f,
                "ControlFrame::Monitor {{ actor_ty_id: {actor_ty_id}, ident: {ident:02x?}, key: {key} }}"
            ),
        }
    }
}
