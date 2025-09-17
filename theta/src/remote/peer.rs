use std::{
    any::type_name,
    fmt::Display,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use dashmap::DashMap;
use futures::channel::oneshot;
use iroh::{NodeAddr, PublicKey};
use rustc_hash::FxBuildHasher;
use serde::{Deserialize, Serialize};
use theta_flume::unbounded_with_id;
use tokio::{select, task_local};
use tracing::{debug, error, trace, warn};

use crate::{
    actor::{Actor, ActorId},
    actor_ref::AnyActorRef,
    base::{DebugActorId, DebugActorIdent, Ident, MonitorError},
    context::{LookupError, RootContext},
    ellipsed,
    message::MsgPack,
    prelude::ActorRef,
    remote::{
        base::{ActorTypeId, Cancel, Key, RemoteError, Tag},
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
// type Key = u64;
// type Key = u64;

/// Local peer handle for the current node's remote actor system.
#[derive(Debug, Clone)]
pub(crate) struct LocalPeer(Arc<LocalPeerInner>);

/// Handle to a remote peer in the distributed actor system.
#[derive(Debug, Clone)]
pub struct Peer(Arc<PeerInner>);

// /// Imported remote actor reference with its originating peer.
// #[derive(Debug)]
// pub(crate) struct Import<A: Actor> {
//     pub(crate) peer: Peer,
//     pub(crate) actor: ActorRef<A>,
// }

#[derive(Debug)]
struct LocalPeerInner {
    public_key: PublicKey,
    network: Network,
    peers: DashMap<PublicKey, Peer, FxBuildHasher>,
    imports: DashMap<ActorId, AnyImport, FxBuildHasher>,
}

type PendingRecvReplies = DashMap<Key, oneshot::Sender<(Peer, Vec<u8>)>, FxBuildHasher>;
type PendingLookups = DashMap<Key, oneshot::Sender<Result<Vec<u8>, LookupError>>, FxBuildHasher>;
type PendingMonitors = DashMap<Key, oneshot::Sender<Result<RxStream, MonitorError>>, FxBuildHasher>;

#[derive(Debug)]
struct PeerInner {
    public_key: PublicKey,
    conn: PreparedConn,

    next_key: AtomicU64,
    pending_recv_replies: PendingRecvReplies,
    pending_lookups: PendingLookups,
    pending_monitors: PendingMonitors,

    cancel: Cancel,
}

// #[derive(Debug)]
// struct PeerState {}

#[derive(Debug)]
struct AnyImport {
    pub(crate) peer: Peer,
    pub(crate) any_actor: Arc<dyn AnyActorRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum InitFrame {
    Import {
        actor_id: ActorId,
    },
    Monitor {
        key: Key,
        mb_err: Option<MonitorError>,
    },
}

// Only the lookup checks the actor_ty, after that consider it as checked.
// Locality suggest, even forward target is once exported-imported, which makes type check is not required.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum ControlFrame {
    Reply {
        key: Key,
        reply_bytes: Vec<u8>,
    },
    Forward {
        ident: Ident,
        tag: Tag, // todo Embed Tag to the bytes
        bytes: Vec<u8>,
    },
    LookupReq {
        key: Key,
        actor_ty_id: ActorTypeId,
        ident: Ident,
    },
    LookupResp {
        key: Key,
        res: Result<Vec<u8>, LookupError>,
    },
    Monitor {
        key: Key,
        actor_ty_id: ActorTypeId,
        ident: Ident,
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
                            error!("failed to accept conn: {e}");
                            continue;
                        }
                        Ok(x) => x,
                    };
                    debug!("incoming connection from peer {}", ellipsed!(&public_key));

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

    pub(crate) fn get_or_connect_peer(&self, public_key: PublicKey) -> Peer {
        match self.peer_entry(&public_key) {
            dashmap::Entry::Vacant(v) => {
                let conn = self
                    .0
                    .network
                    .connect_and_prepare(NodeAddr::new(public_key));

                let peer = Peer::new(public_key, conn);

                v.insert(peer.clone());

                peer
            }
            dashmap::Entry::Occupied(o) => o.get().clone(),
        }
    }

    #[cfg(feature = "monitor")]
    pub(crate) fn get_imported_peer(&self, actor_id: &ActorId) -> Option<Peer> {
        let import = self.0.imports.get(actor_id)?;

        Some(import.peer.clone())
    }

    pub(crate) fn get_import_public_key(&self, actor_id: &ActorId) -> Option<PublicKey> {
        let import = self.0.imports.get(actor_id)?;

        Some(import.peer.public_key())
    }

    // /// Import will always spawn a new actor, but it may not connected to the remote peer successfully.
    // pub(crate) fn import<A: Actor>(
    //     &self,
    //     actor_id: ActorId,
    //     public_key: PublicKey,
    // ) -> Result<ActorRef<A>, RemoteError> {
    //     let peer = self.get_or_connect_peer(public_key);

    //     Ok(peer.import(actor_id))
    // }

    /// Return None if the actor is imported but of different type.
    pub(crate) fn get_or_import_actor<A: Actor>(
        &self,
        actor_id: ActorId,
        peer: impl FnOnce() -> Peer, // todo Make it return ref
    ) -> Option<ActorRef<A>> {
        match self.actor_entry(&actor_id) {
            dashmap::Entry::Vacant(v) => {
                let peer = peer();
                // ! If peer get canceled here, imported actor will immediately get removed.
                // ? Is there any way to avoid that?
                let actor = peer.import_impl(actor_id);

                v.insert(AnyImport {
                    peer,
                    any_actor: Arc::new(actor.clone()),
                });

                Some(actor)
            }
            dashmap::Entry::Occupied(o) => {
                let any_actor = o.get().any_actor.clone();

                match any_actor.as_any().downcast_ref::<ActorRef<A>>() {
                    None => {
                        error!(
                            "failed to downcast imported actor {actor_id} to actor {}",
                            type_name::<A>()
                        );
                        None
                    }
                    Some(actor) => Some(actor.clone()),
                }
            }
        }
    }

    fn peer_entry(&self, public_key: &PublicKey) -> dashmap::Entry<'_, PublicKey, Peer> {
        self.0.peers.entry(*public_key)
    }

    // #[allow(dead_code)]
    // fn get_peer(&self, public_key: &PublicKey) -> Option<Peer> {
    //     self.0.peers.get(public_key).map(|p| p.clone())
    // }

    fn add_peer(&self, public_key: PublicKey, peer: Peer) {
        trace!(
            host =%ellipsed!(&public_key),
            "adding peer",
        );

        if self.0.peers.insert(public_key, peer).is_some() {
            warn!(
                "peer {} replaced, may require inspection",
                ellipsed!(&public_key)
            );
        } else {
            debug!("peer {} added", ellipsed!(&public_key));
        }
    }

    fn remove_peer(&self, public_key: &PublicKey) {
        trace!(
            host =%ellipsed!(&public_key),
            "removing peer",
        );
        match self.peer_entry(public_key) {
            dashmap::Entry::Vacant(_) => {
                warn!(
                    "no peer {} to remove, may require inspection",
                    ellipsed!(public_key)
                ); // Since there are two task per peer
            }
            dashmap::Entry::Occupied(o) => {
                let peer = o.remove();
                peer.0.cancel.cancel();
                debug!("peer {} removed and canceled", ellipsed!(&public_key));
            }
        }
    }

    fn actor_entry(&self, actor_id: &ActorId) -> dashmap::Entry<'_, ActorId, AnyImport> {
        self.0.imports.entry(*actor_id)
    }

    // An actor could be imported from only one peer for now
    fn remove_actor(&self, actor_id: &ActorId) {
        trace!(%actor_id, "removing remote actor from imports");
        if self.0.imports.remove(actor_id).is_none() {
            warn!("no remote actor {actor_id} in imports, may require inspection");
        } else {
            debug!("remote actor {actor_id} removed from imports");
        }
    }
}

impl Peer {
    /// No need of task_local PEER
    pub async fn send_reply(&self, key: Key, reply_bytes: Vec<u8>) -> Result<(), RemoteError> {
        self.send_control_frame(&ControlFrame::Reply { key, reply_bytes })
            .await
    }

    pub(crate) fn new(public_key: PublicKey, conn: PreparedConn) -> Self {
        let this = Self(Arc::new(PeerInner {
            public_key,
            conn,
            next_key: AtomicU64::new(1),
            pending_recv_replies: DashMap::default(),
            pending_lookups: DashMap::default(),
            pending_monitors: DashMap::default(),
            cancel: Cancel::new(),
        }));

        tokio::spawn({
            let this = this.clone();

            async move {
                trace!(
                    host =%this,
                    "starting peer",
                );
                let mut control_rx = match this.0.conn.control_rx().await {
                    Err(e) => {
                        error!("failed to get control_rx: {e}");
                        return LocalPeer::inst().remove_peer(&this.0.public_key);
                    }
                    Ok(rx) => rx,
                };

                let mut init_buf = Vec::new();
                loop {
                    let mut control_buf = Vec::new(); // ephemeral buffer
                    select! {
                        biased;
                        control_frame_res = control_rx.recv_frame_into(&mut control_buf) => {
                            if let Err(e) = control_frame_res {
                                break warn!("failed to receive control frame: {e}");
                            }
                            #[cfg(feature = "verbose")]
                            debug!("received control frame {} bytes from {this}", control_buf.len());

                            let frame: ControlFrame = match postcard::from_bytes(&control_buf) {
                                Err(e) => {
                                    error!("failed to deserialize frame: {e}");
                                    control_buf.clear();
                                    continue;
                                }
                                Ok(frame) => frame,
                            };
                            #[cfg(feature = "verbose")]
                            debug!("deserialized control frame from {this}: {frame}");

                            match frame {
                                ControlFrame::Reply { key, reply_bytes } => {
                                    this.process_reply(key, reply_bytes).await
                                }
                                ControlFrame::Forward { ident, tag, bytes } => {
                                    this.process_forward(ident, tag, bytes).await
                                }
                                ControlFrame::LookupReq {
                                    key,
                                    actor_ty_id,
                                    ident,
                                } => this.process_lookup_req(key, actor_ty_id, ident).await,
                                ControlFrame::LookupResp { res, key } => {
                                    this.process_lookup_resp(key, res).await
                                }
                                ControlFrame::Monitor {
                                    key,
                                    actor_ty_id,
                                    ident,
                                } => this.process_monitor(key, actor_ty_id, ident).await,
                            }
                        }
                        in_stream_res = this.0.conn.accept_uni() => {
                            let mut in_stream = match in_stream_res {
                                Err(e) => break error!("failed to accept stream: {e}"),
                                Ok(s) => s,
                            };
                            debug!("accepted stream from {this}");

                            if let Err(e) = in_stream.recv_frame_into(&mut init_buf).await {
                                break error!("failed to receive initial frame from stream: {e}");
                            }
                            debug!("received initial frame {} bytes from {this}", init_buf.len());

                            let init_frame: InitFrame = match postcard::from_bytes(&init_buf) {
                                Err(e) => {
                                    error!("failed to deserialize lookup message: {e}");
                                    init_buf.clear();
                                    continue;
                                }
                                Ok(frame) => frame,
                            };
                            debug!("deserialized initial frame from {this}: {init_frame}",);

                            match init_frame {
                                InitFrame::Import { actor_id } => {
                                    let Ok(actor) =
                                        RootContext::lookup_any_local_unchecked(actor_id)
                                    else {
                                        error!("local actor reference not found for ident: {actor_id}");
                                        init_buf.clear();
                                        continue;
                                    };

                                    actor.spawn_export_task(this.clone(), in_stream);
                                }
                                InitFrame::Monitor { mb_err, key } => {
                                    let Some((_, tx)) = this.0.pending_monitors.remove(&key) else {
                                        warn!("monitoring key {key} not found");
                                        init_buf.clear();
                                        continue;
                                    };

                                    let res = match mb_err {
                                        Some(e) => Err(e),
                                        None => Ok(in_stream),
                                    };

                                    if tx.send(res).is_err() {
                                        warn!("failed to send stream for monitoring request {key}");
                                    }
                                }
                            }

                            init_buf.clear();
                        }
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

    pub(crate) fn is_canceled(&self) -> bool {
        self.0.cancel.is_canceled()
    }

    // ? Does sync (locking) the peer not to be removed or canceled have any benefit?
    fn import_impl<A: Actor>(&self, actor_id: ActorId) -> ActorRef<A> {
        let (msg_tx, msg_rx) = unbounded_with_id::<MsgPack<A>>(actor_id);

        tokio::spawn({
            let this = self.clone();
            let cancel = self.0.cancel.clone();

            PEER.scope(self.clone(), async move {
                trace!(
                    actor = %DebugActorId::<A>::new(actor_id),
                    host =%this,
                    "starting imported remote actor",
                );

                let mut out_stream = match this.0.conn.open_uni().await {
                    Err(e) => {
                        error!(
                            "failed to open stream to import actor {} {actor_id} from {this}: {e}",
                            type_name::<A>()
                        );
                        return LocalPeer::inst().remove_actor(&actor_id);
                    }
                    Ok(stream) => stream,
                };

                let init_frame = InitFrame::Import { actor_id };

                let bytes = match postcard::to_stdvec(&init_frame) {
                    Err(e) => {
                        error!("failed to serialize lookup message: {e}");
                        return LocalPeer::inst().remove_actor(&actor_id);
                    }
                    Ok(bytes) => bytes,
                };

                trace!(
                    actor = %DebugActorId::<A>::new(actor_id),
                    host =%this,
                    "sending import initial frame",
                );

                if let Err(e) = out_stream.send_frame(&bytes).await {
                    error!("failed to send lookup message: {e}");
                    return LocalPeer::inst().remove_actor(&actor_id);
                }

                loop {
                    let (msg, k) = select! {
                        biased;
                        mb_msg_k = msg_rx.recv() => match mb_msg_k {
                            None => break warn!("failed to receive message for remote actor {} {actor_id} at {this}: local channel closed",
                                type_name::<A>()
                            ),
                            Some(msg_k) => msg_k,
                        },
                        _ = cancel.canceled() => {
                            break warn!("stopped awaiting messages for remote actor {} {actor_id} from {this}: peer disconnected",
                                type_name::<A>()
                            );
                        }
                    };

                    #[cfg(feature = "verbose")]
                    trace!(
                        msg = ?(std::mem::discriminant(&msg), &k),
                        actor = %DebugActor::<A>::new(actor_id),
                        host =%this,
                        "sending remote message",
                    );
                    let Some(k_dto) = k.into_dto().await else {
                        break error!("failed to convert continuation to DTO for remote actor {} {actor_id} at {this}",
                            type_name::<A>()
                        );
                    };

                    let dto: MsgPackDto<A> = (msg, k_dto);
                    let msg_k_bytes = match postcard::to_stdvec(&dto) {
                        Err(e) => break error!("failed to serialize message for remote actor {} {actor_id} at {this}: {e}",
                            type_name::<A>()
                        ),
                        Ok(bytes) => bytes,
                    };

                    if let Err(e) = out_stream.send_frame(&msg_k_bytes).await {
                        break warn!(
                            "failed to send message to remote actor {} {actor_id} at {this}: {e}",
                            type_name::<A>()
                        );
                    }
                }
                debug!("stopped imported remote actor {} {actor_id} from {this}",
                    type_name::<A>()
                );

                LocalPeer::inst().remove_actor(&actor_id);
            })
        });

        ActorRef(msg_tx)
    }

    pub(crate) fn arrange_recv_reply(
        &self,
        reply_bytes_tx: oneshot::Sender<(Peer, Vec<u8>)>,
    ) -> Key {
        if self.0.cancel.is_canceled() {
            warn!("peer is canceled, cannot arrange recv reply");
        }

        let key = self.next_key();

        self.0.pending_recv_replies.insert(key, reply_bytes_tx);

        key
    }

    /// No need of task_local PEER
    pub(crate) async fn send_forward(
        &self,
        ident: Ident,
        tag: Tag,
        bytes: Vec<u8>,
    ) -> Result<(), RemoteError> {
        self.send_control_frame(&ControlFrame::Forward { ident, tag, bytes })
            .await
    }

    #[cfg(feature = "monitor")]
    pub(crate) async fn monitor<A: Actor>(
        &self,
        ident: Ident,
        tx: UpdateTx<A>,
    ) -> Result<(), RemoteError> {
        use crate::base::DebugActorIdent;

        let key = self.next_key();
        let (stream_tx, stream_rx) = oneshot::channel();

        self.0.pending_monitors.insert(key, stream_tx);

        trace!(
            actor = %DebugActorIdent::<A>::new(&ident),
            host =%self,
            key,
            "sending monitoring request",
        );

        let frame = ControlFrame::Monitor {
            actor_ty_id: A::IMPL_ID,
            ident,
            key,
        };

        self.send_control_frame(&frame).await?;

        let mut in_stream = tokio::time::timeout(Duration::from_secs(5), stream_rx).await???;
        debug!(
            "received monitoring stream of remote actor {} from {self}",
            type_name::<A>()
        );

        tokio::spawn({
            let this = self.clone();

            PEER.scope(this, async move {
                loop {
                    let mut buf = Vec::new();
                    if let Err(e) = in_stream.recv_frame_into(&mut buf).await {
                        break error!("failed to receive update frame from remote actor {}: {e}",
                            type_name::<A>()
                        );
                    }

                    let update = match postcard::from_bytes::<Update<A>>(&buf) {
                        Err(e) => {
                            warn!("failed to deserialize update frame {} bytes from remote actor {}: {e}",
                                buf.len(),
                                type_name::<A>()
                            );
                            continue;
                        }
                        Ok(update) => update,
                    };

                    if let Err(e) = tx.send(update) {
                        break warn!("failed to send update: {e}");
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

        self.0.pending_lookups.insert(key, tx);
        trace!(
            actor = %DebugActorIdent::<A>::new(&ident),
            host =%self,
            key,
            "sending lookup request",
        );
        self.send_control_frame(&ControlFrame::LookupReq {
            actor_ty_id: A::IMPL_ID,
            ident,
            key,
        })
        .await?;

        let resp = tokio::time::timeout(Duration::from_secs(5), rx).await??;
        debug!("received lookup response {key} from {self}: {resp:?}");

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
    async fn send_control_frame(&self, frame: &ControlFrame) -> Result<(), RemoteError> {
        let bytes = postcard::to_stdvec(frame).map_err(RemoteError::SerializeError)?;

        self.0.conn.send_frame(&bytes).await?;

        Ok(())
    }

    async fn process_reply(&self, key: Key, reply_bytes: Vec<u8>) {
        let Some((_, reply_bytes_tx)) = self.0.pending_recv_replies.remove(&key) else {
            return warn!("reply key not found: {key}");
        };

        if reply_bytes_tx.send((self.clone(), reply_bytes)).is_err() {
            warn!("failed to send reply");
        }
    }

    async fn process_forward(&self, ident: Ident, tag: Tag, bytes: Vec<u8>) {
        let actor = match RootContext::lookup_any_local_unchecked(&ident) {
            Err(e) => {
                return warn!("failed to lookup local actor {ident:02x?} for forwarding: {e}");
            }
            Ok(actor) => actor,
        };

        if let Err(e) = actor.send_tagged_bytes(tag, bytes) {
            error!("failed to send tagged bytes: {e}");
        }
    }

    async fn process_lookup_req(&self, key: Key, actor_ty_id: ActorTypeId, ident: Ident) {
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
        debug!("processed lookup response {key} to {self} for ident: {ident:02x?}: {resp:?}");

        let bytes = match postcard::to_stdvec(&resp) {
            Err(e) => return error!("failed to serialize lookup response {key}: {e}"),
            Ok(b) => b,
        };

        if let Err(e) = self.0.conn.send_frame(&bytes).await {
            error!("failed to send lookup response: {e}");
        }
    }

    async fn process_lookup_resp(&self, key: Key, lookup_res: Result<Vec<u8>, LookupError>) {
        let Some((_, tx)) = self.0.pending_lookups.remove(&key) else {
            return warn!("lookup key {key} not found");
        };

        if tx.send(lookup_res).is_err() {
            warn!("failed to send lookup response {key}");
        }
    }

    async fn process_monitor(&self, key: Key, actor_ty_id: ActorTypeId, ident: Ident) {
        let _actor = match RootContext::lookup_any_local(actor_ty_id, &ident) {
            Err(e) => {
                warn!("failed to lookup for remote monitoring for ident {ident:02x?}: {e}");
                let init_frame = InitFrame::Monitor {
                    mb_err: Some(e.into()),
                    key,
                };

                if let Err(e) = self.open_uni_with(init_frame).await {
                    error!("failed to open stream for monitoring: {e}");
                }

                return;
            }
            Ok(actor) => actor,
        };

        #[cfg(feature = "monitor")]
        {
            let hdl = {
                let mb_hdl = HDLS.get(&_actor.id()).map(|e| e.clone());

                match mb_hdl {
                    None => {
                        warn!("no monitoring handle found for ident {ident:02x?}");
                        let init_frame = InitFrame::Monitor {
                            mb_err: Some(LookupError::NotFound.into()),
                            key,
                        };

                        if let Err(e) = self.open_uni_with(init_frame).await {
                            error!("failed to open stream for monitoring error: {e}");
                        }

                        return;
                    }
                    Some(h) => h,
                }
            };

            let init_frame = InitFrame::Monitor { mb_err: None, key };

            let out_stream = match self.open_uni_with(init_frame).await {
                Err(e) => {
                    return error!("failed to open stream for monitoring actor {ident:?}: {e}");
                }
                Ok(out_stream) => out_stream,
            };

            if let Err(e) = _actor.monitor_as_bytes(self.clone(), hdl, out_stream) {
                error!("failed to monitor actor {ident:02x?} as bytes: {e}")
            }
        }
    }

    async fn open_uni_with(&self, init_frame: InitFrame) -> Result<TxStream, RemoteError> {
        let mut out_stream = self.0.conn.open_uni().await?;

        let init_bytes = postcard::to_stdvec(&init_frame).map_err(RemoteError::SerializeError)?;

        out_stream.send_frame(&init_bytes).await?;

        Ok(out_stream)
    }

    fn next_key(&self) -> Key {
        self.0.next_key.fetch_add(1, Ordering::Relaxed)
    }
}

impl Display for Peer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let public_key = self.public_key();
        let b = public_key.as_bytes();
        write!(
            f,
            "Peer({:02x}{:02x}{:02x}...{:02x}{:02x}{:02x})",
            b[0], b[1], b[2], b[29], b[30], b[31]
        )
    }
}

impl LocalPeerInner {
    pub fn new(network: Network) -> Self {
        Self {
            public_key: network.public_key(),
            network,
            peers: DashMap::default(),
            imports: DashMap::default(),
        }
    }
}

impl Display for InitFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InitFrame::Import { actor_id } => {
                write!(f, "InitFrame::Import {{ actor_id: {actor_id} }}")
            }
            InitFrame::Monitor { key, mb_err } => {
                write!(f, "InitFrame::Monitor {{ key: {key}, mb_err: {mb_err:?} }}")
            }
        }
    }
}

impl Display for ControlFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlFrame::Reply { key, reply_bytes } => {
                write!(
                    f,
                    "ControlFrame::Reply {{ reply_bytes: {} bytes, key: {key} }}",
                    reply_bytes.len()
                )
            }
            ControlFrame::Forward { ident, tag, bytes } => write!(
                f,
                "ControlFrame::Forward {{ ident: {ident:02x?}, tag: {tag:?}, bytes: {} bytes }}",
                bytes.len()
            ),
            ControlFrame::LookupReq {
                key,
                actor_ty_id,
                ident,
            } => write!(
                f,
                "ControlFrame::LookupReq {{ actor_ty_id: {actor_ty_id}, ident: {ident:02x?}, key: {key} }}"
            ),
            ControlFrame::LookupResp { key, res } => match res {
                Ok(bytes) => write!(
                    f,
                    "ControlFrame::LookupResp {{ res: Ok({} bytes), key: {key} }}",
                    bytes.len()
                ),
                Err(e) => write!(
                    f,
                    "ControlFrame::LookupResp {{ res: Err({e}), key: {key} }}"
                ),
            },
            ControlFrame::Monitor {
                key,
                actor_ty_id,
                ident,
            } => write!(
                f,
                "ControlFrame::Monitor {{ actor_ty_id: {actor_ty_id}, ident: {ident:02x?}, key: {key} }}"
            ),
        }
    }
}
