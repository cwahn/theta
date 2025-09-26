use std::{
    any::type_name,
    fmt::Display,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU32, Ordering},
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
    base::{BindingError, Hex, Ident, MonitorError},
    context::RootContext,
    message::MsgPack,
    prelude::ActorRef,
    remote::{
        base::{ActorTypeId, Key, RemoteError, Tag},
        network::{Network, PreparedConn, RxStream, TxStream},
        serde::MsgPackDto,
    },
};

#[cfg(feature = "monitor")]
use crate::monitor::{HDLS, Update, UpdateTx};

// todo LocalPeer Drop guard

/// Global singleton instance of the local peer for remote communication.
pub(crate) static LOCAL_PEER: OnceLock<LocalPeer> = OnceLock::new();

task_local! {
    pub(crate) static PEER: Peer;
}

/// Local peer handle for the current node's remote actor system.
#[derive(Debug, Clone)]
pub(crate) struct LocalPeer(Arc<LocalPeerInner>);

/// Handle to a remote peer in the distributed actor system.
#[derive(Debug, Clone)]
pub struct Peer(Arc<PeerInner>);

#[derive(Debug)]
struct LocalPeerInner {
    public_key: PublicKey,
    network: Network,
    peers: DashMap<PublicKey, Peer, FxBuildHasher>,
    imports: DashMap<ActorId, AnyImport, FxBuildHasher>,
}

type PendingRecvReplies = DashMap<Key, oneshot::Sender<(Peer, Vec<u8>)>, FxBuildHasher>;
type PendingLookups = DashMap<Key, oneshot::Sender<Result<Vec<u8>, BindingError>>, FxBuildHasher>; // ? Do I need key for this?
type PendingMonitors = DashMap<Key, oneshot::Sender<Result<RxStream, MonitorError>>, FxBuildHasher>; // ? Do I need key for this?

#[derive(Debug)]
struct PeerInner {
    public_key: PublicKey,
    conn: PreparedConn,

    next_key: AtomicU32,
    pending_recv_replies: PendingRecvReplies,
    pending_lookups: PendingLookups,
    pending_monitors: PendingMonitors,

    favored_peer: OnceLock<Peer>,
}

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
        tag: Tag,
        bytes: Vec<u8>,
    },
    LookupReq {
        key: Key,
        actor_ty_id: ActorTypeId,
        ident: Ident,
    },
    LookupResp {
        key: Key,
        res: Result<Vec<u8>, BindingError>,
    },
    Monitor {
        key: Key,
        actor_ty_id: ActorTypeId,
        ident: Ident,
    },
}

// Implementations

impl LocalPeer {
    #[allow(dead_code)]
    pub fn init(endpoint: iroh::Endpoint) {
        let this = LocalPeer(Arc::new(LocalPeerInner::new(Network::new(endpoint))));

        tokio::spawn({
            let this = this.clone();

            async move {
                trace!(
                    public_key = %this.0.public_key,
                    "starting local peer",
                );
                loop {
                    let (public_key, conn) = match this.0.network.accept_and_prepare().await {
                        Err(err) => {
                            error!(%err, "failed to accept conn");
                            continue;
                        }
                        Ok(x) => x,
                    };
                    debug!(
                        public_key = %public_key,
                        "accepted incoming connection",
                    );

                    match this.0.peers.entry(public_key) {
                        dashmap::Entry::Occupied(mut o) => {
                            if this.0.public_key > public_key {
                                trace!(%public_key, "handling unfavored incoming connection");
                                let favored_peer = o.get().clone();

                                favored_peer.run_unfavored(conn);
                            } else {
                                let peer = Peer::new(public_key, conn);

                                trace!(%peer, "replacing with favored");
                                let unfavored_peer = o.insert(peer.clone()); // Old peer will not searched anymore

                                unfavored_peer
                                    .0
                                    .favored_peer
                                    .set(peer.clone())
                                    .expect("favored peer could be set only once here");

                                // if unfavored_peer.0.cancel.cancel() {
                                //     error!(%unfavored_peer, "failed to cancel already canceled");
                                //     continue; // ? Will no more access to the next_key and maps of old outgoing peer?
                                // }

                                peer.0.next_key.store(
                                    unfavored_peer.0.next_key.load(Ordering::Acquire),
                                    Ordering::Release,
                                );

                                Self::move_map(
                                    &peer.0.pending_recv_replies,
                                    &unfavored_peer.0.pending_recv_replies,
                                );
                                Self::move_map(
                                    &peer.0.pending_lookups,
                                    &unfavored_peer.0.pending_lookups,
                                );
                                Self::move_map(
                                    &peer.0.pending_monitors,
                                    &unfavored_peer.0.pending_monitors,
                                );
                            }
                        }
                        dashmap::Entry::Vacant(v) => {
                            let peer = Peer::new(public_key, conn);

                            trace!(%peer, "adding");
                            v.insert(peer);
                        }
                    }
                }
            }
        });

        LOCAL_PEER
            .set(this)
            .expect("local peer should initialize only once");
    }

    pub(crate) fn inst() -> &'static LocalPeer {
        LOCAL_PEER.get().expect("local peer is not initialized")
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

                trace!(%peer, "adding");
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
                let actor = peer.import(actor_id);

                v.insert(AnyImport {
                    peer,
                    any_actor: Arc::new(actor.clone()),
                });

                Some(actor)
            }
            dashmap::Entry::Occupied(o) => {
                let any_actor = o.get().any_actor.clone();

                match any_actor.as_any() {
                    None => None,
                    Some(b) => match b.downcast::<ActorRef<A>>() {
                        Err(_) => None,
                        Ok(actor) => Some(*actor),
                    },
                }
            }
        }
    }

    fn peer_entry(&self, public_key: &PublicKey) -> dashmap::Entry<'_, PublicKey, Peer> {
        self.0.peers.entry(*public_key)
    }

    fn remove_peer(&self, peer: &Peer) {
        trace!(%peer, "removing disconnected");
        match self.peer_entry(&peer.public_key()) {
            dashmap::Entry::Vacant(_) => {
                error!(%peer, "no such peer to remove"); // May require inspection
            }
            dashmap::Entry::Occupied(o) => {
                let peer = o.remove();
                // peer.0.cancel.cancel();
                debug!(%peer, "removed and canceled");
            }
        }
    }

    fn actor_entry(&self, actor_id: &ActorId) -> dashmap::Entry<'_, ActorId, AnyImport> {
        self.0.imports.entry(*actor_id)
    }

    // An actor could be imported from only one peer for now
    fn remove_actor(&self, actor_id: &ActorId) {
        trace!(actor_id = %Hex(actor_id.as_bytes()), "removing remote actor from imports");
        if self.0.imports.remove(actor_id).is_none() {
            error!(actor_id = %Hex(actor_id.as_bytes()), "no such remote actor to remove"); // May require inspection
        } else {
            debug!(actor_id = %Hex(actor_id.as_bytes()), "removed remote actor");
        }
    }

    fn move_map<K: Eq + std::hash::Hash + Copy, V>(
        dst: &DashMap<K, V, FxBuildHasher>,
        src: &DashMap<K, V, FxBuildHasher>,
    ) {
        for key in src.iter().map(|r| *r.key()) {
            if let Some((k, v)) = src.remove(&key) {
                dst.insert(k, v);
            }
        }
    }
}

impl Peer {
    pub(crate) fn new(public_key: PublicKey, conn: PreparedConn) -> Self {
        let inst = Self(Arc::new(PeerInner {
            public_key,
            conn,
            next_key: AtomicU32::default(),
            pending_recv_replies: DashMap::default(),
            pending_lookups: DashMap::default(),
            pending_monitors: DashMap::default(),

            // cancel: Cancel::new(),
            favored_peer: OnceLock::new(),
        }));
        trace!(peer = %inst, %public_key, "creating");

        inst.clone().run();

        inst
    }

    pub(crate) fn public_key(&self) -> PublicKey {
        self.0.public_key
    }

    // pub(crate) fn is_canceled(&self) -> bool {
    //     self.0.cancel.is_canceled()
    // }

    pub(crate) fn arrange_recv_reply(
        &self,
        reply_bytes_tx: oneshot::Sender<(Peer, Vec<u8>)>,
    ) -> Key {
        let key = self.next_key();
        #[cfg(feature = "verbose")]
        trace!(
            host = %self,
            key,
            "arranging to receive reply",
        );
        self.0.pending_recv_replies.insert(key, reply_bytes_tx);

        key
    }

    /// No need of task_local PEER
    pub async fn send_reply(&self, key: Key, reply_bytes: Vec<u8>) -> Result<(), RemoteError> {
        self.send_control_frame(&ControlFrame::Reply { key, reply_bytes })
            .await
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
        let key = self.next_key();
        let (stream_tx, stream_rx) = oneshot::channel();

        self.0.pending_monitors.insert(key, stream_tx);

        trace!(
            actor = format_args!("{}({})", type_name::<A>(), Hex(&ident)),
            host = %self,
            key,
            "sending monitoring request",
        );
        self.send_control_frame(&ControlFrame::Monitor {
            actor_ty_id: A::IMPL_ID,
            ident,
            key,
        })
        .await?;

        let mut in_stream = tokio::time::timeout(Duration::from_secs(5), stream_rx).await???;
        debug!(
            actor = format_args!("{}({})", type_name::<A>(), Hex(&ident)),
            host = %self,
            key,
            "received monitoring stream",
        );

        // ! The result might comes in from the another favored incoming connection.
        // ! Log might display wrong host instance
        tokio::spawn({
            let this = self.clone();
            // todo At this point, replacing peer is required

            PEER.scope(this, async move {
                trace!(
                    actor = format_args!("{}({})", type_name::<A>(), Hex(&ident)),
                    host = %PEER.get(),
                    "starting to monitor",
                );

                loop {
                    let mut buf = Vec::new();
                    if let Err(err) = in_stream.recv_frame_into(&mut buf).await {
                        break warn!(
                            actor = format_args!("{}({})", type_name::<A>(), Hex(&ident)),
                            host = %PEER.get(),
                            %err,
                            "failed to receive update frame",
                        );
                    }

                    let update = match postcard::from_bytes::<Update<A>>(&buf) {
                        Err(err) => {
                            error!(
                                actor = format_args!("{}({})", type_name::<A>(), Hex(&ident)),
                                host = %PEER.get(),
                                %err,
                                "failed to deserialize update frame"
                            );
                            continue;
                        }
                        Ok(update) => update,
                    };

                    if tx.send(update).is_err() {
                        warn!(
                            actor = format_args!("{}({})", type_name::<A>(), Hex(&ident)),
                            host = %PEER.get(),
                            err = "update channel closed",
                            "failed to send update",
                        );
                    }
                }
            })
        });

        Ok(())
    }

    pub(crate) async fn lookup<A: Actor>(
        &self,
        ident: Ident,
    ) -> Result<Result<ActorRef<A>, BindingError>, RemoteError> {
        let key = self.next_key();
        let (tx, rx) = oneshot::channel();

        self.0.pending_lookups.insert(key, tx);
        trace!(
            actor = format_args!("{}({})", type_name::<A>(), Hex(&ident)),
            host =  %self,
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

        // ? Is there any way to avoid this check?
        let peer = match self.0.favored_peer.get() {
            None => self.clone(),
            Some(p) => p.clone(),
        };

        debug!(
            actor = format_args!("{}({})", type_name::<A>(), Hex(&ident)),
            host = %peer,
            key,
            resp = %match &resp { // todo Remove allocation
                Err(err) => format!("Err({err})"),
                Ok(b) => format!("Ok({} bytes)", b.len()),
            },
            "received lookup response",
        );

        let bytes = match resp {
            Err(e) => return Ok(Err(e)),
            Ok(bytes) => bytes,
        };

        let actor = PEER
            .sync_scope(peer, || postcard::from_bytes::<ActorRef<A>>(&bytes))
            .map_err(RemoteError::DeserializeError)?;

        Ok(Ok(actor))
    }

    fn run(self) {
        tokio::spawn({
            async move {
                trace!(from = %self, "starting to accept streams");
                // let control_rx = select! {
                //     biased;
                //     control_rx_res = self.0.conn.control_rx() => match control_rx_res {
                //         Err(err) => {
                //             warn!(from = %self, %err, "failed to get control_rx");
                //             return LocalPeer::inst().remove_peer(&self);
                //         }
                //         Ok(control_rx) => control_rx,
                //     },
                //     _ = self.0.cancel.canceled() => match self.0.favored_peer.get() {
                //         None => {
                //             warn!(from = %self, "stopped canceled peer before getting control_rx");
                //             return LocalPeer::inst().remove_peer(&self);
                //         }
                //         Some(_) => return debug!(peer = %self, "stopped canceled unfavored")
                //     }

                // };

                let control_rx = match self.0.conn.control_rx().await {
                    Err(err) => {
                        warn!(from = %self, %err, "failed to get control_rx");
                        return LocalPeer::inst().remove_peer(&self);
                    }
                    Ok(control_rx) => control_rx,
                };

                self.clone().spawn_recv_frame(control_rx);

                let mut buf = Vec::new();
                loop {
                    let mut in_stream = match self.0.conn.accept_uni().await {
                        Err(err) => break warn!(from = %self, %err, "failed to accept stream"),
                        Ok(s) => s,
                    };
                    debug!(from = %self, "accepted stream");

                    if let Err(err) = in_stream.recv_frame_into(&mut buf).await {
                        break warn!(from = %self, %err, "failed to receive initial frame");
                    }
                    debug!(len = buf.len(), from = %self, "received initial frame");

                    let init_frame: InitFrame = match postcard::from_bytes(&buf) {
                        Err(err) => {
                            error!(from = %self, %err, "failed to deserialize initial frame");
                            buf.clear();
                            continue;
                        }
                        Ok(frame) => frame,
                    };
                    debug!(from = %self, frame = %init_frame, "deserialized initial frame");

                    match init_frame {
                        InitFrame::Import { actor_id } => {
                            let Ok(actor) =
                                RootContext::lookup_any_local_unchecked_impl(actor_id.as_bytes())
                            else {
                                error!(from = %self, actor_id = %Hex(actor_id.as_bytes()), "local actor to export not found");
                                buf.clear();
                                continue;
                            };

                            if let Err(err) = actor.spawn_export_task(self.clone(), in_stream) {
                                warn!(from = %self, actor_id = %Hex(actor_id.as_bytes() ), %err, "failed to spawn export task");
                            };
                        }
                        InitFrame::Monitor { mb_err, key } => {
                            let Some((_, tx)) = self.0.pending_monitors.remove(&key) else {
                                error!(from = %self, key, "monitoring key not found");
                                buf.clear();
                                continue;
                            };

                            let res = match mb_err {
                                Some(e) => Err(e),
                                None => Ok(in_stream),
                            };

                            if tx.send(res).is_err() {
                                error!(from = %self, key, "failed to send monitoring stream");
                            }
                        }
                    }

                    buf.clear();
                }

                LocalPeer::inst().remove_peer(&self);
            }
        });
    }

    fn run_unfavored(self, unfavored_conn: PreparedConn) {
        tokio::spawn({
            async move {
                trace!(from = %self, "accepting unfavored control_rx");
                // let control_rx = select! {
                //     biased;
                //     control_rx_res = unfavored_conn.control_rx() => {
                //         match control_rx_res {
                //             Err(err) => {
                //                 warn!(from = %self, %err, "failed to get control_rx");
                //                 return LocalPeer::inst().remove_peer(&self);
                //             }
                //             Ok(control_rx) => control_rx,
                //         }
                //     }
                //     _ = self.0.cancel.canceled() => {
                //         match self.0.favored_peer.get() {
                //             None => {
                //                 warn!(from = %self, "stopped canceled peer before getting control_rx");
                //                 return LocalPeer::inst().remove_peer(&self);
                //             }
                //             Some(_) => unreachable!("self is known to be no unfavored")
                //         }
                //     }
                // };

                let control_rx = match unfavored_conn.control_rx().await {
                    Err(err) => {
                        return warn!(from = %self, %err, "failed to get control_rx");
                    }
                    Ok(control_rx) => control_rx,
                };

                self.clone().spawn_recv_frame(control_rx);
            }
        });
    }

    fn spawn_recv_frame(self, mut control_rx: RxStream) {
        tokio::spawn({
            async move {
                trace!(from = %self, "starting to receive frames");
                loop {
                    let mut buf = Vec::new(); // ephemeral buffer
                    // select! {
                    //     biased;
                    //     recv_res = control_rx.recv_frame_into(&mut buf) => {
                    //         if let Err(err) = recv_res {
                    //             break warn!(from = %self, %err, "failed to receive control frame");
                    //         }
                    //     }
                    //     _ = self.0.cancel.canceled() => match self.0.favored_peer.get() {
                    //         None => return warn!(
                    //             from = %self,
                    //             "stopped canceled peer while receiving frames"
                    //         ),
                    //         Some(_) => return debug!(
                    //             peer = %self,
                    //             "stopped canceled unfavored while receiving frames"
                    //         ),
                    //     }
                    // }

                    if let Err(err) = control_rx.recv_frame_into(&mut buf).await {
                        break warn!(from = %self, %err, "failed to receive control frame");
                    }

                    #[cfg(feature = "verbose")]
                    debug!(from = %self, len = buf.len(), "received control frame");

                    let frame: ControlFrame = match postcard::from_bytes(&buf) {
                        Err(err) => {
                            error!(from = %self, %err, "failed to deserialize control frame");
                            buf.clear();
                            continue;
                        }
                        Ok(frame) => frame,
                    };
                    #[cfg(feature = "verbose")]
                    debug!(from = %self, %frame, "deserialized control frame");

                    match frame {
                        ControlFrame::Reply { key, reply_bytes } => {
                            self.process_reply(key, reply_bytes).await
                        }
                        ControlFrame::Forward { ident, tag, bytes } => {
                            self.process_forward(ident, tag, bytes).await
                        }
                        ControlFrame::LookupReq {
                            key,
                            actor_ty_id,
                            ident,
                        } => self.process_lookup_req(key, actor_ty_id, ident).await,
                        ControlFrame::LookupResp { res, key } => {
                            self.process_lookup_resp(key, res).await
                        }
                        ControlFrame::Monitor {
                            key,
                            actor_ty_id,
                            ident,
                        } => self.process_monitor(key, actor_ty_id, ident).await,
                    }
                }
            }
        });
    }

    // ? Does sync (locking) the peer not to be removed or canceled have any benefit?
    // This will not get called for unfavored outgoing connection since it will happen only after receiving LookupReq or any data containing actor_id
    fn import<A: Actor>(&self, actor_id: ActorId) -> ActorRef<A> {
        let (msg_tx, msg_rx) = unbounded_with_id::<MsgPack<A>>(actor_id);

        tokio::spawn({
            let this = self.clone();
            // let cancel = self.0.cancel.clone();

            PEER.scope(self.clone(), async move {
                trace!(
                    actor = format_args!("{}({actor_id})", type_name::<A>()),
                    host = %this,
                    "starting imported remote",
                );

                let mut out_stream = match this.0.conn.open_uni().await {
                    Err(err) => {
                        warn!(
                            actor = format_args!("{}({actor_id})", type_name::<A>()),
                            host = %this,
                            %err,
                            "failed to open stream to import",
                        );

                        return LocalPeer::inst().remove_actor(&actor_id);
                    }
                    Ok(stream) => stream,
                };

                let init_frame = InitFrame::Import { actor_id };

                let bytes = match postcard::to_stdvec(&init_frame) {
                    Err(err) => {
                        error!(
                            actor = format_args!("{}({actor_id})", type_name::<A>()),
                            host = %this,
                            %err,
                            "failed to serialize import initial frame",
                        );
                        return LocalPeer::inst().remove_actor(&actor_id);
                    }
                    Ok(bytes) => bytes,
                };

                trace!(
                    actor = format_args!("{}({actor_id})", type_name::<A>()),
                    host =%this,
                    "sending import initial frame",
                );
                if let Err(err) = out_stream.send_frame(&bytes).await {
                    warn!(
                        actor = format_args!("{}({actor_id})", type_name::<A>()),
                        host = %this,
                        %err,
                        "failed to send import initial frame",
                    );
                    return LocalPeer::inst().remove_actor(&actor_id);
                }

                loop {
                    let (msg, k) = select! {
                        biased;
                        mb_msg_k = msg_rx.recv() => match mb_msg_k {
                            None => break debug!(
                                actor = format_args!("{}({actor_id})", type_name::<A>()),
                                host = %this,
                                "remote actor message channel closed",
                            ),
                            Some(msg_k) => msg_k,
                        },
                        // ! May be not needed.
                        // Just wait for stopped could be better option.
                        // _ = cancel.canceled() => {
                        //     // ! If the un-favored outgoing connection is canceled, this is the place most likely to be notified.
                        //     // ? How can I tell from the regular disconnection?
                        //     // -> by checking if there is replacement incoming connection.
                        //     // And at that point, the key point is to reuse the existing msg_rx

                        //     // ? How to know if this is just a regular disconnection or replacement with favored incoming connection?
                        //     break warn!(
                        //         actor = format_args!("{}({actor_id})", type_name::<A>()),
                        //         host = %this,
                        //         "stopped disconnected remote"
                        //     );
                        // }

                        _ = out_stream.stopped() => break warn!(
                            actor = format_args!("{}({actor_id})", type_name::<A>()),
                            host = %this,
                            "stopped disconnected remote",
                        ),
                    };

                    #[cfg(feature = "verbose")]
                    trace!(
                        msg = ?(std::mem::discriminant(&msg), &k),
                        target = format_args!("{}({})", type_name::<A>(), Hex(actor_id.as_bytes())),
                        host =%this,
                        "sending remote",
                    );
                    let Some(k_dto) = k.into_dto().await else {
                        break error!(
                            target = format_args!("{}({actor_id})", type_name::<A>()),
                            host = %this,
                            "failed to convert continuation to DTO",
                        );
                    };

                    let dto: MsgPackDto<A> = (msg, k_dto);
                    let msg_k_bytes = match postcard::to_stdvec(&dto) {
                        Err(err) => {
                            error!(
                                target = format_args!("{}({actor_id})", type_name::<A>()),
                                host = %this,
                                %err,
                                "failed to serialize message for remote",
                            );
                            continue;
                        }
                        Ok(bytes) => bytes,
                    };

                    if let Err(err) = out_stream.send_frame(&msg_k_bytes).await {
                        break warn!(
                            target = format_args!("{}({actor_id})", type_name::<A>()),
                            host = %this,
                            %err,
                            "failed to send message to remote",
                        );
                    }
                }
                debug!(
                    actor = format_args!("{}({actor_id})", type_name::<A>()),
                    host = %this,
                    "stopped imported remote",
                );

                LocalPeer::inst().remove_actor(&actor_id);
            })
        });

        ActorRef(msg_tx)
    }

    /// No need of task_local PEER
    async fn send_control_frame(&self, frame: &ControlFrame) -> Result<(), RemoteError> {
        let bytes = postcard::to_stdvec(frame).map_err(RemoteError::SerializeError)?;
        self.0.conn.send_frame(&bytes).await?;

        Ok(())
    }

    // ! All those below is not likely to be called in Peer with nonfavored outgoing connection since it handles incoming frames.

    async fn process_reply(&self, key: Key, reply_bytes: Vec<u8>) {
        let Some((_, reply_bytes_tx)) = self.0.pending_recv_replies.remove(&key) else {
            return error!(host = %self, key, "reply key not found");
        };

        if reply_bytes_tx.send((self.clone(), reply_bytes)).is_err() {
            warn!(host = %self, key, "failed to send reply");
        }
    }

    async fn process_forward(&self, ident: Ident, tag: Tag, bytes: Vec<u8>) {
        let actor = match RootContext::lookup_any_local_unchecked_impl(&ident) {
            Err(err) => {
                return error!(
                    ident = %Hex(&ident),
                    host = %self,
                    %err,
                    "failed to lookup local actor for forwarding",
                );
            }
            Ok(actor) => actor,
        };

        if let Err(err) = actor.send_tagged_bytes(tag, bytes) {
            warn!(
                ident = %Hex(&ident),
                host = %self,
                %err,
                "failed to send tagged bytes to local actor",
            );
        }
    }

    async fn process_lookup_req(&self, key: Key, actor_ty_id: ActorTypeId, ident: Ident) {
        let resp = match RootContext::lookup_any_local_impl(actor_ty_id, &ident) {
            Err(e) => ControlFrame::LookupResp { res: Err(e), key },
            Ok(any_actor) => match PEER.sync_scope(self.clone(), || any_actor.serialize()) {
                Err(e) => ControlFrame::LookupResp { res: Err(e), key },
                Ok(actor) => ControlFrame::LookupResp {
                    res: Ok(actor),
                    key,
                },
            },
        };
        debug!(
            ident = %Hex(&ident),
            host = %self,
            key,
            %resp,
            "processed lookup request",
        );

        let bytes = match postcard::to_stdvec(&resp) {
            Err(err) => {
                return error!(
                    ident = %Hex(&ident),
                    host = %self,
                    key,
                    %err,
                    "failed to serialize lookup response",
                );
            }
            Ok(b) => b,
        };

        if let Err(err) = self.0.conn.send_frame(&bytes).await {
            warn!(
                ident = %Hex(&ident),
                host = %self,
                key,
                %err,
                "failed to send lookup response",
            );
        }
    }

    async fn process_lookup_resp(&self, key: Key, lookup_res: Result<Vec<u8>, BindingError>) {
        let Some((_, tx)) = self.0.pending_lookups.remove(&key) else {
            return error!(host = %self, key, "lookup key not found");
        };

        if tx.send(lookup_res).is_err() {
            warn!(host = %self, key, "failed to send lookup response");
        }
    }

    async fn process_monitor(&self, key: Key, actor_ty_id: ActorTypeId, ident: Ident) {
        let _actor = match RootContext::lookup_any_local_impl(actor_ty_id, &ident) {
            Err(err) => {
                warn!(
                    ident = %Hex(&ident),
                    host = %self,
                    %err,
                    "failed to lookup local actor for remote monitoring request",
                );
                let init_frame = InitFrame::Monitor {
                    mb_err: Some(err.into()),
                    key,
                };

                if let Err(err) = self.open_uni_with(init_frame).await {
                    warn!(
                        ident = %Hex(&ident),
                        host = %self,
                        %err,
                        "failed to open stream for monitoring error",
                    );
                }

                return;
            }
            Ok(actor) => actor,
        };

        #[cfg(feature = "monitor")]
        {
            let Some(id) = _actor.id() else {
                return warn!(
                    ident = %Hex(&ident),
                    host = %self,
                    "failed to monitor terminated",
                );
            };

            let hdl = {
                match HDLS.get(&id).map(|h| h.clone()) {
                    None => {
                        warn!(
                            ident = %Hex(&ident),
                            host = %self,
                            "failed to monitor not found",
                        );
                        let init_frame = InitFrame::Monitor {
                            mb_err: Some(BindingError::NotFound.into()),
                            key,
                        };

                        if let Err(err) = self.open_uni_with(init_frame).await {
                            warn!(
                                ident = %Hex(&ident),
                                host = %self,
                                %err,
                                "failed to open monitoring stream",
                            );
                        }

                        return;
                    }
                    Some(hdl) => hdl,
                }
            };

            let init_frame = InitFrame::Monitor { mb_err: None, key };

            let out_stream = match self.open_uni_with(init_frame).await {
                Err(e) => {
                    return warn!(
                        ident = %Hex(&ident),
                        host = %self,
                        %e,
                        "failed to open monitoring stream",
                    );
                }
                Ok(out_stream) => out_stream,
            };

            if let Err(e) = _actor.monitor_as_bytes(self.clone(), hdl, out_stream) {
                error!(
                    ident = %Hex(&ident),
                    host = %self,
                    %e,
                    "failed to monitor as bytes",
                );
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
        // todo Secure salted display
        write!(f, "Peer({:p})", Arc::as_ptr(&self.0))
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
                write!(
                    f,
                    "InitFrame::Import {{ actor_id: {} }}",
                    Hex(actor_id.as_bytes())
                )
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
                "ControlFrame::Forward {{ ident: {}, tag: {tag:?}, bytes: {} bytes }}",
                Hex(ident),
                bytes.len()
            ),
            ControlFrame::LookupReq {
                key,
                actor_ty_id,
                ident,
            } => write!(
                f,
                "ControlFrame::LookupReq {{ actor_ty_id: {actor_ty_id}, ident: {}, key: {key} }}",
                Hex(ident),
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
                "ControlFrame::Monitor {{ actor_ty_id: {actor_ty_id}, ident: {}, key: {key} }}",
                Hex(ident)
            ),
        }
    }
}
