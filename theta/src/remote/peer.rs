use std::{
    any::type_name,
    fmt::Display,
    sync::{Arc, OnceLock},
    time::Duration,
};

use futures::{FutureExt, channel::oneshot, future::Either, future::select};
use iroh::{
    Endpoint, PublicKey,
    endpoint::{RecvStream, SendStream},
};
use pin_utils::pin_mut;
use serde::{Deserialize, Serialize};
use theta_flume::{Receiver, unbounded_with_id};
use tracing::{debug, error, trace, warn};

#[cfg(feature = "monitor")]
use crate::monitor::{HDLS, Update, UpdateTx};
use crate::{
    actor::{Actor, ActorId},
    actor_ref::AnyActorRef,
    base::{BindingError, Hex, Ident, MonitorError},
    compat,
    context::RootContext,
    message::{Continuation, MsgPack},
    prelude::ActorRef,
    remote::{
        base::{ActorTypeId, RemoteError, Tag},
        network::{Network, NetworkError, PreparedConn, RecvFrameExt, SendFrameExt},
        serde::{ActorRefDto, ContinuationDto, MsgPackDto},
    },
};

compat_task_local! {
    #[allow(clippy::upper_case_acronyms)] pub (crate) static PEER : Peer;
}

/// Global singleton instance of the local peer for remote communication.
pub(crate) static LOCAL_PEER: OnceLock<LocalPeer> = OnceLock::new();

/// Local peer handle for the current node's remote actor system.
#[derive(Debug, Clone)]
pub(crate) struct LocalPeer(Arc<LocalPeerInner>);

/// Handle to a remote peer in the distributed actor system.
#[derive(Debug, Clone)]
pub struct Peer(Arc<PeerInner>);

impl LocalPeer {
    pub fn init(endpoint: Endpoint) {
        let this = Self(Arc::new(LocalPeerInner::new(Network::new(endpoint))));

        compat::spawn({
            let this = this.clone();

            async move {
                trace!(public_key = % this.0.public_key, "starting local peer",);

                loop {
                    let (public_key, conn) = match this.0.network.accept_and_prepare().await {
                        Err(err) => {
                            error!(% err, "failed to accept conn");

                            continue;
                        }
                        Ok(x) => x,
                    };
                    debug!(public_key = % public_key, "accepted incoming connection",);

                    match this.0.peers.entry(public_key) {
                        compat::Entry::Occupied(mut o) => {
                            if this.0.public_key > public_key {
                                trace!(% public_key, "handling unfavored incoming connection");
                                o.get().clone().run_unfavored(conn);

                                continue;
                            }

                            let peer = Peer::new(public_key, conn);

                            trace!(% peer, "replacing with favored");
                            let unfavored_peer = o.insert(peer.clone());

                            unfavored_peer
                                .0
                                .favored_peer
                                .set(peer.clone())
                                .expect("favored peer could be set only once here");
                            unfavored_peer.0.conn.close(b"superseded");
                        }
                        compat::Entry::Vacant(v) => {
                            let peer = Peer::new(public_key, conn);

                            trace!(% peer, "adding");
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

    pub(crate) fn inst() -> &'static Self {
        LOCAL_PEER.get().expect("local peer is not initialized")
    }

    pub(crate) fn public_key(&self) -> PublicKey {
        self.0.public_key
    }

    pub(crate) fn endpoint(&self) -> &Endpoint {
        &self.0.network.endpoint
    }

    pub(crate) fn get_or_connect_peer(&self, public_key: PublicKey) -> Peer {
        match self.peer_entry(&public_key) {
            compat::Entry::Occupied(o) => o.get().clone(),
            compat::Entry::Vacant(v) => {
                let conn = self.0.network.connect_and_prepare(public_key);
                let peer = Peer::new(public_key, conn);

                trace!(% peer, "adding");
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
        peer: impl FnOnce() -> Peer,
    ) -> Option<ActorRef<A>> {
        match self.actor_entry(&actor_id) {
            compat::Entry::Vacant(v) => {
                let peer = peer();
                let actor = peer.import(actor_id);

                v.insert(AnyImport {
                    peer,
                    any_actor: Arc::new(actor.clone()),
                });

                Some(actor)
            }
            compat::Entry::Occupied(o) => {
                let any_actor = o.get().any_actor.clone();

                any_actor
                    .as_any()?
                    .downcast::<ActorRef<A>>()
                    .ok()
                    .map(|a| *a)
            }
        }
    }

    fn peer_entry(&self, public_key: &PublicKey) -> compat::Entry<'_, PublicKey, Peer> {
        self.0.peers.entry(*public_key)
    }

    fn remove_peer(&self, peer: &Peer) {
        trace!(% peer, "removing disconnected");

        match self.peer_entry(&peer.public_key()) {
            compat::Entry::Vacant(_) => {
                error!(% peer, "no such peer to remove");
            }
            compat::Entry::Occupied(o) => {
                if !Arc::ptr_eq(&o.get().0, &peer.0) {
                    return debug!(% peer, "skipping removal: superseded by path migration");
                }

                let peer = o.remove();
                debug!(% peer, "removed and canceled");
            }
        }
    }

    fn actor_entry(&self, actor_id: &ActorId) -> compat::Entry<'_, ActorId, AnyImport> {
        self.0.imports.entry(*actor_id)
    }

    fn remove_actor(&self, actor_id: &ActorId) {
        trace!(actor_id = % Hex(actor_id.as_bytes()), "removing remote actor from imports");

        if self.0.imports.remove(actor_id).is_none() {
            error!(actor_id = % Hex(actor_id.as_bytes()), "no such remote actor to remove");
        } else {
            debug!(actor_id = % Hex(actor_id.as_bytes()), "removed remote actor");
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

        trace!(peer = % inst, % public_key, "creating");
        inst.clone().run();

        inst
    }

    pub(crate) fn public_key(&self) -> PublicKey {
        self.0.public_key
    }

    /// No need of `task_local` PEER
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
            actor = format_args!("{}({})", type_name::< A > (), Hex(&ident)), host = % self,
            "sending monitoring request via bi-stream",
        );
        let (mut send_half, mut recv_half) = self.0.conn.open_bi().await?;

        let init_frame = InitFrame::Monitor {
            actor_ty_id: A::IMPL_ID,
            ident,
        };
        let init_bytes = postcard::to_stdvec(&init_frame).map_err(RemoteError::SerializeError)?;

        send_half.send_frame(init_bytes).await?;

        let mut status = [0u8; 1];

        compat::timeout(Duration::from_secs(5), recv_half.read_exact(&mut status))
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
            actor = format_args!("{}({})", type_name::< A > (), Hex(&ident)), host = % self,
            "received monitoring stream",
        );

        compat::spawn({
            let this = self.clone();

            PEER.scope(this, async move {
                trace!(
                    actor = format_args!("{}({})", type_name::< A > (), Hex(&ident)), host = %
                    PEER.get(), "starting to monitor",
                );
                let mut buf = Vec::new();

                loop {
                    buf.clear();

                    if let Err(err) = recv_half.recv_frame_into(&mut buf).await {
                        break warn!(
                            actor = format_args!("{}({})", type_name::< A > (), Hex(&ident)),
                            host = % PEER.get(), % err, "failed to receive update frame",
                        );
                    }

                    let update = match postcard::from_bytes::<Update<A>>(&buf) {
                        Err(err) => {
                            error!(
                                actor = format_args!("{}({})", type_name::< A > (),
                                Hex(&ident)), host = % PEER.get(), % err,
                                "failed to deserialize update frame"
                            );

                            continue;
                        }
                        Ok(update) => update,
                    };

                    if tx.send(update).is_err() {
                        break warn!(
                            actor = format_args!("{}({})", type_name::< A > (), Hex(&ident)),
                            host = % PEER.get(), err = "update channel closed",
                            "stopped monitoring",
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
            actor = format_args!("{}({})", type_name::< A > (), Hex(&ident)), host = % self,
            "sending lookup request via bi-stream",
        );
        let (mut send_half, mut recv_half) = self.0.conn.open_bi().await?;

        let init_frame = InitFrame::Lookup {
            actor_ty_id: A::IMPL_ID,
            ident,
        };
        let init_bytes = postcard::to_stdvec(&init_frame).map_err(RemoteError::SerializeError)?;

        send_half.send_frame(init_bytes).await?;
        drop(send_half);

        let mut buf = Vec::new();

        compat::timeout(Duration::from_secs(5), recv_half.recv_frame_into(&mut buf))
            .await
            .map_err(|_| RemoteError::Timeout)??;

        let resp: Result<Vec<u8>, BindingError> =
            postcard::from_bytes(&buf).map_err(RemoteError::DeserializeError)?;

        let peer = match self.0.favored_peer.get() {
            None => self.clone(),
            Some(p) => p.clone(),
        };
        debug!(
            actor = format_args!("{}({})", type_name::< A > (), Hex(&ident)), host = % peer, resp =
            % match &resp { Err(err) => format!("Err({err})"), Ok(b) => format!("Ok({} bytes)", b
            .len()), }, "received lookup response",
        );

        let bytes = match resp {
            Err(e) => return Ok(Err(e)),
            Ok(bytes) => bytes,
        };

        let dto: ActorRefDto =
            postcard::from_bytes(&bytes).map_err(RemoteError::DeserializeError)?;

        Ok(PEER.sync_scope(peer, || dto.try_into()))
    }

    fn run(self) {
        self.clone().spawn_control_reader(self.0.conn.clone());
        compat::spawn({
            async move {
                trace!(from = % self, "accepting bi-streams");
                loop {
                    let (reply_tx, in_stream) = match self.0.conn.accept_bi().await {
                        Err(err) => break warn!(from = % self, % err, "failed to accept bi-stream"),
                        Ok(bi) => bi,
                    };
                    debug!(from = % self, "accepted bi-stream");

                    Self::spawn_bi_handler(self.clone(), reply_tx, in_stream);
                }

                LocalPeer::inst().remove_peer(&self);
            }
        });
    }

    /// Accept control frames on a given connection.
    /// Runs as a dedicated task: awaits `control_rx` (which may involve connection setup),
    /// then reads Forward frames in a loop. When the stream dies, the peer is considered
    /// disconnected. For unfavored connections, only this reader is spawned.
    fn spawn_control_reader(self, conn: PreparedConn) {
        compat::spawn({
            async move {
                let mut control_rx = match conn.control_rx().await {
                    Err(err) => {
                        return warn!(from = % self, % err, "failed to accept control stream");
                    }
                    Ok(rx) => rx,
                };

                trace!(from = % self, "control stream reader started");
                let mut buf = Vec::new();

                loop {
                    buf.clear();

                    if let Err(err) = control_rx.recv_frame_into(&mut buf).await {
                        break warn!(from = % self, % err, "control stream ended");
                    }

                    let frame: ControlFrame = match postcard::from_bytes(&buf) {
                        Err(err) => {
                            error!(from = % self, % err, "failed to deserialize control frame");

                            continue;
                        }
                        Ok(frame) => frame,
                    };
                    #[cfg(feature = "verbose")]
                    debug!(from = % self, % frame, "received control frame");

                    match frame {
                        ControlFrame::Forward { ident, tag, bytes } => {
                            self.process_forward(ident, tag, bytes);
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
    fn spawn_bi_handler(peer: Self, reply_tx: SendStream, in_stream: RecvStream) {
        compat::spawn(async move {
            let mut buf = Vec::new();
            let mut in_stream = in_stream;
            let mut reply_tx = reply_tx;

            match compat::timeout(Duration::from_secs(5), in_stream.recv_frame_into(&mut buf)).await
            {
                Err(_) => {
                    return warn!(
                        from = % peer,
                        "timeout receiving initial frame on bi-stream, dropping dead stream"
                    );
                }
                Ok(Err(err)) => {
                    return warn!(
                        from = % peer, % err, "failed to receive initial frame on bi-stream"
                    );
                }
                Ok(Ok(())) => {}
            }

            let init_frame: InitFrame = match postcard::from_bytes(&buf) {
                Err(err) => {
                    return error!(from = % peer, % err, "failed to deserialize initial frame");
                }
                Ok(frame) => frame,
            };

            match init_frame {
                InitFrame::Import { actor_id } => {
                    let Ok(actor) =
                        RootContext::lookup_any_local_unchecked_impl(actor_id.as_bytes())
                    else {
                        return error!(
                            from = % peer, actor_id = % Hex(actor_id.as_bytes()),
                            "local actor to export not found"
                        );
                    };

                    if let Err(err) = actor.spawn_export_task(peer.clone(), in_stream, reply_tx) {
                        warn!(
                            from = % peer, actor_id = % Hex(actor_id.as_bytes()), % err,
                            "failed to spawn export task"
                        );
                    }
                }
                InitFrame::Lookup { actor_ty_id, ident } => {
                    let resp = match RootContext::lookup_any_local_impl(actor_ty_id, &ident) {
                        Err(e) => Err(e),
                        Ok(any_actor) => PEER.sync_scope(peer.clone(), || any_actor.serialize()),
                    };
                    debug!(ident = % Hex(&ident), host = % peer, "processed lookup request",);

                    let bytes = match postcard::to_stdvec(&resp) {
                        Err(err) => {
                            return error!(
                                ident = % Hex(&ident), host = % peer, % err,
                                "failed to serialize lookup response"
                            );
                        }
                        Ok(b) => b,
                    };

                    if let Err(err) = reply_tx.send_frame(bytes).await {
                        warn!(
                            ident = % Hex(&ident), host = % peer, % err,
                            "failed to send lookup response"
                        );
                    }
                }
                #[cfg(feature = "monitor")]
                InitFrame::Monitor { actor_ty_id, ident } => {
                    Self::handle_monitor_request(peer, actor_ty_id, ident, reply_tx).await;
                }
                #[cfg(not(feature = "monitor"))]
                _ => {
                    error!(from = % peer, "unexpected InitFrame variant on bi-stream");
                }
            }
        });
    }

    #[cfg(feature = "monitor")]
    async fn handle_monitor_request(
        peer: Self,
        actor_ty_id: ActorTypeId,
        ident: Ident,
        mut reply_tx: SendStream,
    ) {
        let actor = match RootContext::lookup_any_local_impl(actor_ty_id, &ident) {
            Err(err) => {
                warn!(
                    ident = % Hex(&ident), host = % peer, % err,
                    "failed to lookup local actor for remote monitoring request",
                );

                if let Err(err) = reply_tx.write_all(&[0x00]).await {
                    return error!(
                        ident = % Hex(&ident), host = % peer, % err,
                        "failed to send monitor error status"
                    );
                }

                let monitor_err = MonitorError::from(err);

                let err_bytes = match postcard::to_stdvec(&monitor_err) {
                    Ok(b) => b,
                    Err(err) => {
                        return error!(
                            ident = % Hex(&ident), host = % peer, % err,
                            "failed to serialize monitor error"
                        );
                    }
                };

                if let Err(err) = reply_tx.send_frame(err_bytes).await {
                    error!(
                        ident = % Hex(&ident), host = % peer, % err,
                        "failed to send monitor error frame"
                    );
                }

                return;
            }
            Ok(actor) => actor,
        };

        let Some(id) = actor.id() else {
            return warn!(ident = % Hex(&ident), host = % peer, "failed to monitor terminated",);
        };

        let Some(hdl) = HDLS.get(&id) else {
            warn!(ident = % Hex(&ident), host = % peer, "failed to monitor not found",);

            if let Err(err) = reply_tx.write_all(&[0x00]).await {
                return error!(
                    ident = % Hex(&ident), host = % peer, % err,
                    "failed to send monitor error status"
                );
            }

            let monitor_err = MonitorError::from(BindingError::NotFound);

            let err_bytes = match postcard::to_stdvec(&monitor_err) {
                Err(err) => {
                    return error!(
                        ident = % Hex(&ident), host = % peer, % err,
                        "failed to serialize monitor error"
                    );
                }
                Ok(b) => b,
            };

            if let Err(err) = reply_tx.send_frame(err_bytes).await {
                error!(
                    ident = % Hex(&ident), host = % peer, % err,
                    "failed to send monitor error frame"
                );
            }

            return;
        };

        if reply_tx.write_all(&[0x01]).await.is_err() {
            return warn!(
                ident = % Hex(&ident), host = % peer, "failed to send monitoring ok status",
            );
        }

        if let Err(e) = actor.monitor_as_bytes(peer.clone(), hdl, reply_tx) {
            error!(ident = % Hex(&ident), host = % peer, % e, "failed to monitor as bytes",);
        }
    }

    fn import<A: Actor>(&self, actor_id: ActorId) -> ActorRef<A> {
        let (msg_tx, msg_rx) = unbounded_with_id::<MsgPack<A>>(actor_id);

        compat::spawn({
            let this = self.clone();

            PEER.scope(self.clone(), async move {
                trace!(
                    actor = format_args!("{}({actor_id})", type_name::< A > ()), host = % this,
                    "starting imported remote",
                );

                let Some((mut send_half, mut recv_half, first_msg)) =
                    init_import::<A>(&this, actor_id, &msg_rx).await
                else {
                    return;
                };

                run_import::<A>(
                    &this,
                    actor_id,
                    &msg_rx,
                    &mut send_half,
                    &mut recv_half,
                    first_msg,
                )
                .await;

                debug!(
                    actor = format_args!("{}({actor_id})", type_name::< A > ()), host = % this,
                    "stopped imported remote",
                );

                LocalPeer::inst().remove_actor(&actor_id);
            })
        });

        ActorRef(msg_tx)
    }

    /// No need of `task_local` PEER
    async fn send_control_frame(&self, frame: &ControlFrame) -> Result<(), RemoteError> {
        let bytes = postcard::to_stdvec(frame).map_err(RemoteError::SerializeError)?;

        self.0.conn.send_control_frame(bytes).await?;

        Ok(())
    }

    fn process_forward(&self, ident: Ident, tag: Tag, bytes: Vec<u8>) {
        let actor = match RootContext::lookup_any_local_unchecked_impl(&ident) {
            Err(err) => {
                return error!(
                    ident = % Hex(&ident), host = % self, % err,
                    "failed to lookup local actor for forwarding",
                );
            }
            Ok(actor) => actor,
        };

        if let Err(err) = actor.send_tagged_bytes(tag, bytes) {
            warn!(
                ident = % Hex(&ident), host = % self, % err,
                "failed to send tagged bytes to local actor",
            );
        }
    }
}

async fn init_import<A: Actor>(
    this: &Peer,
    actor_id: ActorId,
    msg_rx: &Receiver<MsgPack<A>>,
) -> Option<(SendStream, RecvStream, MsgPack<A>)> {
    let init_frame = InitFrame::Import { actor_id };

    let init_bytes = match postcard::to_stdvec(&init_frame) {
        Err(err) => {
            error!(
                actor = format_args!("{}({actor_id})", type_name::< A > ()), host = % this, % err,
                "failed to serialize import initial frame",
            );

            LocalPeer::inst().remove_actor(&actor_id);

            return None;
        }
        Ok(bytes) => bytes,
    };

    let Some(first_msg) = msg_rx.recv().await else {
        debug!(
            actor = format_args!("{}({actor_id})", type_name::< A > ()), host = % this,
            "remote actor message channel closed (no messages ever sent)",
        );

        LocalPeer::inst().remove_actor(&actor_id);

        return None;
    };

    let (mut send_half, recv_half) = match this.0.conn.open_bi().await {
        Err(err) => {
            error!(
                actor = format_args!("{}({actor_id})", type_name::< A > ()), host = % this, % err,
                "failed to open bi-stream to import",
            );

            LocalPeer::inst().remove_actor(&actor_id);

            return None;
        }
        Ok(s) => s,
    };

    if let Err(err) = send_half.send_frame(init_bytes).await {
        error!(
            actor = format_args!("{}({actor_id})", type_name::< A > ()), host = % this, % err,
            "failed to send import initial frame",
        );

        LocalPeer::inst().remove_actor(&actor_id);

        return None;
    }

    Some((send_half, recv_half, first_msg))
}

async fn run_import<A: Actor>(
    this: &Peer,
    actor_id: ActorId,
    msg_rx: &Receiver<MsgPack<A>>,
    send_half: &mut SendStream,
    recv_half: &mut RecvStream,
    first_msg: MsgPack<A>,
) {
    let mut pending_first = Some(first_msg);

    loop {
        let Some((msg, k)) =
            recv_msg::<A>(&mut pending_first, msg_rx, send_half, this, actor_id).await
        else {
            break;
        };

        #[cfg(feature = "verbose")]
        trace!(
            msg = ? (std::mem::discriminant(&msg), &k), target =
            format_args!("{}({})", type_name::< A > (), Hex(actor_id.as_bytes())),
            host =% this, "sending remote",
        );

        let (k_dto, bin_reply_tx) = match k {
            Continuation::Reply(tx) => {
                let (bin_reply_tx, bin_reply_rx) = oneshot::channel();

                if tx.send(Box::new(bin_reply_rx)).is_err() {
                    warn!(
                        target = format_args!("{}({actor_id})", type_name::< A >
                        ()), host = % this,
                        "failed to send binary reply rx to caller",
                    );

                    continue;
                }

                (ContinuationDto::Reply, Some(bin_reply_tx))
            }
            other => {
                let Some(dto) = other.into_dto().await else {
                    break error!(
                        target = format_args!("{}({actor_id})", type_name::< A >
                        ()), host = % this, "failed to convert continuation to DTO",
                    );
                };

                (dto, None)
            }
        };

        let dto: MsgPackDto<A> = (msg, k_dto);

        let msg_k_bytes = match postcard::to_stdvec(&dto) {
            Err(err) => {
                error!(
                    target = format_args!("{}({actor_id})", type_name::< A > ()),
                    host = % this, % err, "failed to serialize message for remote",
                );

                continue;
            }
            Ok(bytes) => bytes,
        };

        if let Err(err) = send_half.send_frame(msg_k_bytes).await {
            break warn!(
                target = format_args!("{}({actor_id})", type_name::< A > ()), host =
                % this, % err, "failed to send message to remote",
            );
        }

        if let Some(reply_tx) = bin_reply_tx
            && !handle_reply(this, actor_id, type_name::<A>(), reply_tx, recv_half).await
        {
            break;
        }
    }
}

async fn recv_msg<A: Actor>(
    pending: &mut Option<MsgPack<A>>,
    msg_rx: &Receiver<MsgPack<A>>,
    send_half: &SendStream,
    this: &Peer,
    actor_id: ActorId,
) -> Option<MsgPack<A>> {
    if let Some(msg_k) = pending.take() {
        return Some(msg_k);
    }

    if let Ok(msg_k) = msg_rx.try_recv() {
        return Some(msg_k);
    }

    let recv_fut = msg_rx.recv().fuse();
    let stopped_fut = send_half.stopped().fuse();

    pin_mut!(recv_fut, stopped_fut);

    match select(recv_fut, stopped_fut).await {
        Either::Left((mb_msg_k, _)) => match mb_msg_k {
            None => {
                debug!(
                    actor = format_args!("{}({actor_id})", type_name::< A > ()),
                    host = % this, "remote actor message channel closed",
                );

                None
            }
            Some(msg_k) => Some(msg_k),
        },
        Either::Right((_, _)) => {
            warn!(
                actor = format_args!("{}({actor_id})", type_name::< A > ()),
                host = % this, "stopped disconnected remote",
            );

            None
        }
    }
}

async fn handle_reply(
    this: &Peer,
    actor_id: ActorId,
    actor_ty_name: &str,
    reply_tx: oneshot::Sender<(Peer, Vec<u8>)>,
    recv_half: &mut RecvStream,
) -> bool {
    let mut buf = Vec::new();

    if let Err(err) = recv_half.recv_frame_into(&mut buf).await {
        warn!(
            actor = format_args!("{actor_ty_name}({actor_id})"),
            host = % this, % err, "reply read failed, stopping import",
        );

        return false;
    }

    if reply_tx.send((this.clone(), buf)).is_err() {
        trace!(
            actor = format_args!("{actor_ty_name}({actor_id})"),
            host = % this, "reply oneshot dropped (caller cancelled)",
        );
    }

    true
}

impl Display for Peer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Peer({:p})", Arc::as_ptr(&self.0))
    }
}

impl LocalPeerInner {
    pub fn new(network: Network) -> Self {
        Self {
            public_key: network.public_key(),
            network,
            peers: compat::ConcurrentMap::default(),
            imports: compat::ConcurrentMap::default(),
        }
    }
}

impl Display for InitFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Import { actor_id } => {
                write!(
                    f,
                    "InitFrame::Import {{ actor_id: {} }}",
                    Hex(actor_id.as_bytes())
                )
            }
            Self::Lookup { actor_ty_id, ident } => {
                write!(
                    f,
                    "InitFrame::Lookup {{ actor_ty_id: {actor_ty_id}, ident: {} }}",
                    Hex(ident)
                )
            }
            #[cfg(feature = "monitor")]
            Self::Monitor { actor_ty_id, ident } => {
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
            Self::Forward { ident, tag, bytes } => {
                write!(
                    f,
                    "ControlFrame::Forward {{ ident: {}, tag: {tag:?}, bytes: {} bytes }}",
                    Hex(ident),
                    bytes.len()
                )
            }
        }
    }
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

#[derive(Debug)]
struct LocalPeerInner {
    public_key: PublicKey,
    network: Network,
    peers: compat::ConcurrentMap<PublicKey, Peer>,
    imports: compat::ConcurrentMap<ActorId, AnyImport>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ControlFrame {
    Forward {
        ident: Ident,
        tag: Tag,
        bytes: Vec<u8>,
    },
}
