use std::{
    any::type_name,
    fmt::Display,
    sync::{Arc, OnceLock},
    time::Duration,
};

use crate::compat::{ConcurrentMap, Entry};
use futures::{FutureExt, channel::oneshot, future::Either};
use iroh::{
    PublicKey,
    endpoint::{RecvStream, SendStream},
};
use pin_utils::pin_mut;
use serde::{Deserialize, Serialize};
use theta_flume::unbounded_with_id;
use tracing::{debug, error, trace, warn};

use crate::{
    actor::{Actor, ActorId},
    actor_ref::AnyActorRef,
    base::{BindingError, Hex, Ident, MonitorError},
    context::RootContext,
    message::{Continuation, MsgPack},
    prelude::ActorRef,
    remote::{
        base::{ActorTypeId, RemoteError, Tag},
        network::{Network, NetworkError, PreparedConn, RecvFrameExt, SendFrameExt},
        serde::{ContinuationDto, MsgPackDto},
    },
};

#[cfg(feature = "monitor")]
use crate::monitor::{HDLS, Update, UpdateTx};

// todo LocalPeer Drop guard

/// Global singleton instance of the local peer for remote communication.
pub(crate) static LOCAL_PEER: OnceLock<LocalPeer> = OnceLock::new();

compat_task_local! {
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
    peers: ConcurrentMap<PublicKey, Peer>,
    imports: ConcurrentMap<ActorId, AnyImport>,
}

#[derive(Debug)]
struct PeerInner {
    public_key: PublicKey,
    conn: PreparedConn,
    favored_peer: OnceLock<Peer>,
}

#[derive(Debug, Clone)]
struct AnyImport {
    pub(crate) peer: Peer,
    pub(crate) any_actor: Arc<dyn AnyActorRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum InitFrame {
    Import {
        actor_id: ActorId,
    },
    Lookup {
        actor_ty_id: ActorTypeId,
        ident: Ident,
    },
    #[cfg(feature = "monitor")]
    Monitor {
        actor_ty_id: ActorTypeId,
        ident: Ident,
    },
}

// Only the lookup checks the actor_ty, after that consider it as checked.
// Locality suggest, even forward target is once exported-imported, which makes type check is not required.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum ControlFrame {
    // NOTE: Reply traffic moved to per-actor bi-streams (no longer on control stream)
    Forward {
        ident: Ident,
        tag: Tag,
        bytes: Vec<u8>,
    },
}

// Implementations

impl LocalPeer {
    pub fn init(endpoint: iroh::Endpoint) {
        let this = LocalPeer(Arc::new(LocalPeerInner::new(Network::new(endpoint))));

        crate::compat::spawn({
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
                        Entry::Occupied(mut o) => {
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
                                // In-flight lookups/monitors on old conn complete or timeout on their own bi-streams
                            }
                        }
                        Entry::Vacant(v) => {
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

    pub(crate) fn endpoint(&self) -> &iroh::Endpoint {
        &self.0.network.endpoint
    }

    pub(crate) fn get_or_connect_peer(&self, public_key: PublicKey) -> Peer {
        match self.peer_entry(&public_key) {
            Entry::Occupied(o) => o.get().clone(),
            Entry::Vacant(v) => {
                let conn = self.0.network.connect_and_prepare(public_key);

                let peer = Peer::new(public_key, conn);

                trace!(%peer, "adding");
                v.insert(peer.clone());

                peer
            }
        }
    }

    #[cfg(feature = "monitor")]
    pub(crate) fn get_imported_peer(&self, actor_id: &ActorId) -> Option<Peer> {
        self.0.imports.with(actor_id, |import| import.peer.clone())
    }

    pub(crate) fn get_import_public_key(&self, actor_id: &ActorId) -> Option<PublicKey> {
        self.0
            .imports
            .with(actor_id, |import| import.peer.public_key())
    }

    /// Return None if the actor is imported but of different type.
    pub(crate) fn get_or_import_actor<A: Actor>(
        &self,
        actor_id: ActorId,
        peer: impl FnOnce() -> Peer, // todo Make it return ref
    ) -> Option<ActorRef<A>> {
        match self.actor_entry(&actor_id) {
            Entry::Vacant(v) => {
                let peer = peer();

                let actor = peer.import(actor_id);

                v.insert(AnyImport {
                    peer,
                    any_actor: Arc::new(actor.clone()),
                });

                Some(actor)
            }
            Entry::Occupied(o) => {
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

    fn peer_entry(&self, public_key: &PublicKey) -> Entry<'_, PublicKey, Peer> {
        self.0.peers.entry(*public_key)
    }

    fn remove_peer(&self, peer: &Peer) {
        trace!(%peer, "removing disconnected");
        match self.peer_entry(&peer.public_key()) {
            Entry::Vacant(_) => {
                error!(%peer, "no such peer to remove"); // May require inspection
            }
            Entry::Occupied(o) => {
                let peer = o.remove();
                // peer.0.cancel.cancel();
                debug!(%peer, "removed and canceled");
            }
        }
    }

    fn actor_entry(&self, actor_id: &ActorId) -> Entry<'_, ActorId, AnyImport> {
        self.0.imports.entry(*actor_id)
    }

    // An actor could be imported from only one peer for now
    fn remove_actor(&self, actor_id: &ActorId) {
        trace!(actor_id = %Hex(actor_id.as_bytes()), "removing remote actor from imports");
        match self.0.imports.remove(actor_id) {
            None => error!(actor_id = %Hex(actor_id.as_bytes()), "no such remote actor to remove"), // May require inspection
            Some(_) => debug!(actor_id = %Hex(actor_id.as_bytes()), "removed remote actor"),
        }
    }
}

impl Peer {
    pub(crate) fn new(public_key: PublicKey, conn: PreparedConn) -> Self {
        let inst = Self(Arc::new(PeerInner {
            public_key,
            conn,
            favored_peer: OnceLock::new(),
        }));
        trace!(peer = %inst, %public_key, "creating");

        inst.clone().run();

        inst
    }

    pub(crate) fn public_key(&self) -> PublicKey {
        self.0.public_key
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
        trace!(
            actor = format_args!("{}({})", type_name::<A>(), Hex(&ident)),
            host = %self,
            "sending monitoring request via bi-stream",
        );

        // Open bi-stream and send InitFrame::Monitor
        let (mut send_half, mut recv_half) = self.0.conn.open_bi().await?;
        let init_frame = InitFrame::Monitor {
            actor_ty_id: A::IMPL_ID,
            ident,
        };
        let init_bytes = postcard::to_stdvec(&init_frame).map_err(RemoteError::SerializeError)?;
        send_half.send_frame(init_bytes).await?;

        // Wait for status byte: 0x01 = ok, 0x00 = error (followed by MonitorError frame)
        let mut status = [0u8; 1];
        crate::compat::timeout(Duration::from_secs(5), recv_half.read_exact(&mut status))
            .await
            .map_err(|_| RemoteError::Timeout)?
            .map_err(|e| NetworkError::ReadExactError(Arc::new(e)))?;

        if status[0] == 0x00 {
            let mut buf = Vec::new();
            recv_half.recv_frame_into(&mut buf).await?;
            let err: MonitorError =
                postcard::from_bytes(&buf).map_err(RemoteError::DeserializeError)?;
            return Err(RemoteError::MonitorError(err));
        }

        debug!(
            actor = format_args!("{}({})", type_name::<A>(), Hex(&ident)),
            host = %self,
            "received monitoring stream",
        );

        // Spawn update reader on the recv_half
        crate::compat::spawn({
            let this = self.clone();
            PEER.scope(this, async move {
                trace!(
                    actor = format_args!("{}({})", type_name::<A>(), Hex(&ident)),
                    host = %PEER.get(),
                    "starting to monitor",
                );

                loop {
                    let mut buf = Vec::new();
                    if let Err(err) = recv_half.recv_frame_into(&mut buf).await {
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
        trace!(
            actor = format_args!("{}({})", type_name::<A>(), Hex(&ident)),
            host = %self,
            "sending lookup request via bi-stream",
        );

        // Open bi-stream and send InitFrame::Lookup
        let (mut send_half, mut recv_half) = self.0.conn.open_bi().await?;
        let init_frame = InitFrame::Lookup {
            actor_ty_id: A::IMPL_ID,
            ident,
        };
        let init_bytes = postcard::to_stdvec(&init_frame).map_err(RemoteError::SerializeError)?;
        send_half.send_frame(init_bytes).await?;
        drop(send_half); // Signal we're done sending

        // Wait for response with timeout — stream auto-closes on drop
        let mut buf = Vec::new();
        crate::compat::timeout(Duration::from_secs(5), recv_half.recv_frame_into(&mut buf))
            .await
            .map_err(|_| RemoteError::Timeout)??;

        let resp: Result<Vec<u8>, BindingError> =
            postcard::from_bytes(&buf).map_err(RemoteError::DeserializeError)?;

        let peer = match self.0.favored_peer.get() {
            None => self.clone(),
            Some(p) => p.clone(),
        };

        debug!(
            actor = format_args!("{}({})", type_name::<A>(), Hex(&ident)),
            host = %peer,
            resp = %match &resp {
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
        // Spawn control stream reader as a separate task.
        // This mirrors the original spawn_recv_frame pattern: control_rx setup
        // (which may block on connection establishment) runs independently,
        // so the main accept loop can start accepting bi-streams immediately.
        self.clone().spawn_control_reader(self.0.conn.clone());

        crate::compat::spawn({
            async move {
                trace!(from = %self, "accepting bi-streams");

                loop {
                    let (reply_tx, in_stream) = match self.0.conn.accept_bi().await {
                        Err(err) => {
                            break warn!(from = %self, %err, "failed to accept bi-stream");
                        }
                        Ok(bi) => bi,
                    };
                    Self::spawn_bi_handler(self.clone(), reply_tx, in_stream);
                }

                LocalPeer::inst().remove_peer(&self);
            }
        });
    }

    /// Accept control frames on a given connection.
    /// Runs as a dedicated task: awaits control_rx (which may involve connection setup),
    /// then reads Forward frames in a loop. When the stream dies, the peer is considered
    /// disconnected. For unfavored connections, only this reader is spawned.
    fn spawn_control_reader(self, conn: PreparedConn) {
        crate::compat::spawn({
            async move {
                let mut control_rx = match conn.control_rx().await {
                    Err(err) => {
                        return warn!(from = %self, %err, "failed to accept control stream");
                    }
                    Ok(rx) => rx,
                };

                trace!(from = %self, "control stream reader started");

                loop {
                    let mut buf = Vec::new();
                    if let Err(err) = control_rx.recv_frame_into(&mut buf).await {
                        break warn!(from = %self, %err, "control stream ended");
                    }

                    let frame: ControlFrame = match postcard::from_bytes(&buf) {
                        Err(err) => {
                            error!(from = %self, %err, "failed to deserialize control frame");
                            continue;
                        }
                        Ok(frame) => frame,
                    };
                    #[cfg(feature = "verbose")]
                    debug!(from = %self, %frame, "received control frame");

                    match frame {
                        ControlFrame::Forward { ident, tag, bytes } => {
                            self.process_forward(ident, tag, bytes).await;
                        }
                    }
                }
            }
        });
    }

    /// Accept control frames on an unfavored incoming connection.
    fn run_unfavored(self, unfavored_conn: PreparedConn) {
        self.spawn_control_reader(unfavored_conn);
    }

    /// Spawned handler for incoming bi-streams: Import, Lookup, Monitor
    fn spawn_bi_handler(peer: Peer, reply_tx: SendStream, in_stream: RecvStream) {
        crate::compat::spawn(async move {
            let mut buf = Vec::new();
            let mut in_stream = in_stream;
            let mut reply_tx = reply_tx;

            // Timeout recv_frame to detect dead streams (iroh path migration)
            match crate::compat::timeout(
                Duration::from_secs(5),
                in_stream.recv_frame_into(&mut buf),
            )
            .await
            {
                Err(_) => {
                    return warn!(from = %peer, "timeout receiving initial frame on bi-stream, dropping dead stream");
                }
                Ok(Err(err)) => {
                    return warn!(from = %peer, %err, "failed to receive initial frame on bi-stream");
                }
                Ok(Ok(())) => {}
            }

            let init_frame: InitFrame = match postcard::from_bytes(&buf) {
                Err(err) => {
                    return error!(from = %peer, %err, "failed to deserialize initial frame");
                }
                Ok(frame) => frame,
            };

            match init_frame {
                InitFrame::Import { actor_id } => {
                    let Ok(actor) =
                        RootContext::lookup_any_local_unchecked_impl(actor_id.as_bytes())
                    else {
                        return error!(from = %peer, actor_id = %Hex(actor_id.as_bytes()), "local actor to export not found");
                    };

                    if let Err(err) = actor.spawn_export_task(peer.clone(), in_stream, reply_tx) {
                        warn!(from = %peer, actor_id = %Hex(actor_id.as_bytes()), %err, "failed to spawn export task");
                    }
                }
                InitFrame::Lookup { actor_ty_id, ident } => {
                    let resp = match RootContext::lookup_any_local_impl(actor_ty_id, &ident) {
                        Err(e) => Err(e),
                        Ok(any_actor) => PEER.sync_scope(peer.clone(), || any_actor.serialize()),
                    };
                    debug!(
                        ident = %Hex(&ident),
                        host = %peer,
                        "processed lookup request",
                    );

                    let bytes = match postcard::to_stdvec(&resp) {
                        Err(err) => {
                            return error!(ident = %Hex(&ident), host = %peer, %err, "failed to serialize lookup response");
                        }
                        Ok(b) => b,
                    };

                    if let Err(err) = reply_tx.send_frame(bytes).await {
                        warn!(ident = %Hex(&ident), host = %peer, %err, "failed to send lookup response");
                    }
                }
                #[cfg(feature = "monitor")]
                InitFrame::Monitor { actor_ty_id, ident } => {
                    Self::handle_monitor_request(peer, actor_ty_id, ident, reply_tx).await;
                }
                #[cfg(not(feature = "monitor"))]
                _ => {
                    error!(from = %peer, "unexpected InitFrame variant on bi-stream");
                }
            }
        });
    }

    #[cfg(feature = "monitor")]
    async fn handle_monitor_request(
        peer: Peer,
        actor_ty_id: ActorTypeId,
        ident: Ident,
        mut reply_tx: SendStream,
    ) {
        let _actor = match RootContext::lookup_any_local_impl(actor_ty_id, &ident) {
            Err(err) => {
                warn!(
                    ident = %Hex(&ident),
                    host = %peer,
                    %err,
                    "failed to lookup local actor for remote monitoring request",
                );
                // Status 0x00 = error, followed by serialized MonitorError
                let _ = reply_tx.write_all(&[0x00]).await;
                let err_bytes = match postcard::to_stdvec(&MonitorError::from(err)) {
                    Ok(b) => b,
                    Err(_) => return,
                };
                let _ = reply_tx.send_frame(err_bytes).await;
                return;
            }
            Ok(actor) => actor,
        };

        let Some(id) = _actor.id() else {
            return warn!(
                ident = %Hex(&ident),
                host = %peer,
                "failed to monitor terminated",
            );
        };

        let hdl = {
            match HDLS.get(&id) {
                None => {
                    warn!(
                        ident = %Hex(&ident),
                        host = %peer,
                        "failed to monitor not found",
                    );
                    let _ = reply_tx.write_all(&[0x00]).await;
                    let err_bytes =
                        match postcard::to_stdvec(&MonitorError::from(BindingError::NotFound)) {
                            Ok(b) => b,
                            Err(_) => return,
                        };
                    let _ = reply_tx.send_frame(err_bytes).await;
                    return;
                }
                Some(hdl) => hdl,
            }
        };

        // Status 0x01 = success, then updates flow on the stream
        if reply_tx.write_all(&[0x01]).await.is_err() {
            return warn!(
                ident = %Hex(&ident),
                host = %peer,
                "failed to send monitoring ok status",
            );
        }

        if let Err(e) = _actor.monitor_as_bytes(peer.clone(), hdl, reply_tx) {
            error!(
                ident = %Hex(&ident),
                host = %peer,
                %e,
                "failed to monitor as bytes",
            );
        }
    }

    // ? Does sync (locking) the peer not to be removed or canceled have any benefit?
    // This will not get called for unfavored outgoing connection since it will happen only after receiving LookupReq or any data containing actor_id
    fn import<A: Actor>(&self, actor_id: ActorId) -> ActorRef<A> {
        let (msg_tx, msg_rx) = unbounded_with_id::<MsgPack<A>>(actor_id);

        crate::compat::spawn({
            let this = self.clone();

            PEER.scope(self.clone(), async move {
                trace!(
                    actor = format_args!("{}({actor_id})", type_name::<A>()),
                    host = %this,
                    "starting imported remote",
                );

                let init_frame = InitFrame::Import { actor_id };
                let init_bytes = match postcard::to_stdvec(&init_frame) {
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

                // === Lazy stream: wait for first message before opening bi-stream ===
                let first_msg = match msg_rx.recv().await {
                    None => {
                        debug!(
                            actor = format_args!("{}({actor_id})", type_name::<A>()),
                            host = %this,
                            "remote actor message channel closed (no messages ever sent)",
                        );
                        return LocalPeer::inst().remove_actor(&actor_id);
                    }
                    Some(msg_k) => msg_k,
                };

                let (mut send_half, mut recv_half) = match this.0.conn.open_bi().await {
                    Err(err) => {
                        error!(
                            actor = format_args!("{}({actor_id})", type_name::<A>()),
                            host = %this,
                            %err,
                            "failed to open bi-stream to import",
                        );
                        return LocalPeer::inst().remove_actor(&actor_id);
                    }
                    Ok(s) => s,
                };

                if let Err(err) = send_half.send_frame(init_bytes).await {
                    error!(
                        actor = format_args!("{}({actor_id})", type_name::<A>()),
                        host = %this,
                        %err,
                        "failed to send import initial frame",
                    );
                    return LocalPeer::inst().remove_actor(&actor_id);
                }


                // === Single-task loop: merged send + inline reply read ===
                // No separate reply_reader task. Reply is read inline after sending a Reply-type message.
                let mut pending_first = Some(first_msg);

                loop {

                    let (msg, k) = if let Some(msg_k) = pending_first.take() {
                        msg_k
                    } else {
                        match msg_rx.try_recv() {
                            Ok(msg_k) => msg_k,
                            Err(_) => {
                                let recv_fut = msg_rx.recv().fuse();
                                let stopped_fut = send_half.stopped().fuse();
                                pin_mut!(recv_fut, stopped_fut);
                                match futures::future::select(recv_fut, stopped_fut).await {
                                    Either::Left((mb_msg_k, _)) => match mb_msg_k {
                                        None => break debug!(
                                            actor = format_args!("{}({actor_id})", type_name::<A>()),
                                            host = %this,
                                            "remote actor message channel closed",
                                        ),
                                        Some(msg_k) => msg_k,
                                    },
                                    Either::Right((_, _)) => break warn!(
                                        actor = format_args!("{}({actor_id})", type_name::<A>()),
                                        host = %this,
                                        "stopped disconnected remote",
                                    ),
                                }
                            }
                        }
                    };

                    #[cfg(feature = "verbose")]
                    trace!(
                        msg = ?(std::mem::discriminant(&msg), &k),
                        target = format_args!("{}({})", type_name::<A>(), Hex(actor_id.as_bytes())),
                        host =%this,
                        "sending remote",
                    );

                    // Handle Reply continuation: keep bin_reply_tx for inline delivery
                    let (k_dto, bin_reply_tx) = match k {
                        Continuation::Reply(tx) => {
                            let (bin_reply_tx, bin_reply_rx) = oneshot::channel();

                            if tx.send(Box::new(bin_reply_rx)).is_err() {
                                warn!(
                                    target = format_args!("{}({actor_id})", type_name::<A>()),
                                    host = %this,
                                    "failed to send binary reply rx to caller",
                                );
                                continue;
                            }

                            (ContinuationDto::Reply, Some(bin_reply_tx))
                        }
                        other => {
                            let Some(dto) = other.into_dto().await else {
                                break error!(
                                    target = format_args!("{}({actor_id})", type_name::<A>()),
                                    host = %this,
                                    "failed to convert continuation to DTO",
                                );
                            };
                            (dto, None)
                        }
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

                    if let Err(err) = send_half.send_frame(msg_k_bytes).await {
                        break warn!(
                            target = format_args!("{}({actor_id})", type_name::<A>()),
                            host = %this,
                            %err,
                            "failed to send message to remote",
                        );
                    }

                    // Inline reply: read reply immediately after sending a Reply-type message.
                    // No separate reply_reader task needed.
                    if let Some(reply_tx) = bin_reply_tx {
                        let mut buf = Vec::new();
                        if let Err(err) = recv_half.recv_frame_into(&mut buf).await {
                            break warn!(
                                actor = format_args!("{}({actor_id})", type_name::<A>()),
                                host = %this,
                                %err,
                                "reply read failed, stopping import",
                            );
                        }

                        if reply_tx.send((this.clone(), buf)).is_err() {
                            trace!(
                                actor = format_args!("{}({actor_id})", type_name::<A>()),
                                host = %this,
                                "reply oneshot dropped (caller cancelled)",
                            );
                        }
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

        self.0.conn.send_control_frame(&bytes).await?;

        Ok(())
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
            peers: ConcurrentMap::default(),
            imports: ConcurrentMap::default(),
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
            InitFrame::Lookup { actor_ty_id, ident } => {
                write!(
                    f,
                    "InitFrame::Lookup {{ actor_ty_id: {actor_ty_id}, ident: {} }}",
                    Hex(ident)
                )
            }
            #[cfg(feature = "monitor")]
            InitFrame::Monitor { actor_ty_id, ident } => {
                write!(
                    f,
                    "InitFrame::Monitor {{ actor_ty_id: {actor_ty_id}, ident: {} }}",
                    Hex(ident)
                )
            }
        }
    }
}

impl Display for ControlFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlFrame::Forward { ident, tag, bytes } => write!(
                f,
                "ControlFrame::Forward {{ ident: {}, tag: {tag:?}, bytes: {} bytes }}",
                Hex(ident),
                bytes.len()
            ),
        }
    }
}
