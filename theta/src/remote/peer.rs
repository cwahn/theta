use std::{
    any::type_name,
    fmt::Display,
    marker::PhantomData,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use crate::compat::{ConcurrentMap, Entry};
use futures::{FutureExt, channel::oneshot};
use iroh::{
    PublicKey,
    endpoint::{RecvStream, SendStream},
};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, trace, warn};
use uuid::Uuid;

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
    import_pool: ImportPool,
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

// === Task-eliminated import pool ===

pub(crate) struct PendingFrame {
    bytes: Vec<u8>,
    reply_tx: Option<oneshot::Sender<(Peer, Vec<u8>)>>,
}

enum StreamState {
    NotReady,
    Ready(SendStream, RecvStream),
    Failed,
}

pub(crate) struct ImportMailbox {
    pub(crate) actor_id: ActorId,
    pub(crate) uuid: Uuid,
    pub(crate) peer: Peer,
    conn: PreparedConn,
    init_bytes: Vec<u8>,
    pub(crate) buffer: std::sync::Mutex<Vec<PendingFrame>>,
    stream: tokio::sync::Mutex<StreamState>,
    work_tx: theta_flume::Sender<ActorId>,
    pub(crate) closed: AtomicBool,
    pub(crate) sender_count: AtomicUsize,
}

impl std::fmt::Debug for ImportMailbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportMailbox")
            .field("actor_id", &self.actor_id)
            .field("closed", &self.closed.load(Ordering::Relaxed))
            .finish()
    }
}

// Safe: ImportMailbox is only accessed through Arc and Mutex/AtomicBool.
// Panic in flush workers is independent of actor panic recovery.
impl std::panic::UnwindSafe for ImportMailbox {}
impl std::panic::RefUnwindSafe for ImportMailbox {}

/// Type-erased remote sender that can be stored in ActorRef without knowing A.
/// The PhantomData ensures correct generic behavior; the actual serialization
/// happens in the MsgSender::Remote::send() path where A is known.
pub(crate) struct RemoteSender<A: Actor> {
    pub(crate) mailbox: Arc<ImportMailbox>,
    pub(crate) _phantom: PhantomData<fn(A) -> A>,
}

impl<A: Actor> Clone for RemoteSender<A> {
    fn clone(&self) -> Self {
        self.mailbox.sender_count.fetch_add(1, Ordering::Relaxed);
        RemoteSender {
            mailbox: self.mailbox.clone(),
            _phantom: PhantomData,
        }
    }
}

impl<A: Actor> Drop for RemoteSender<A> {
    fn drop(&mut self) {
        self.mailbox.sender_count.fetch_sub(1, Ordering::Relaxed);
    }
}

impl<A: Actor> std::fmt::Debug for RemoteSender<A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RemoteSender({})", Hex(self.mailbox.actor_id.as_bytes()))
    }
}

impl<A: Actor> RemoteSender<A> {
    /// Eagerly serialize (msg, continuation_dto) and push to the mailbox buffer.
    /// This is called from MsgSender::Remote in the caller's sync context.
    pub(crate) fn send(&self, (msg, k): MsgPack<A>) -> Result<(), theta_flume::SendError<MsgPack<A>>> {
        if self.mailbox.closed.load(Ordering::Acquire) {
            return Err(theta_flume::SendError((msg, k)));
        }

        let (k_dto, reply_tx) = match k {
            Continuation::Nil => (ContinuationDto::Nil, None),
            Continuation::Reply(tx) => {
                let (bin_tx, bin_rx) = oneshot::channel();
                if tx.send(Box::new(bin_rx)).is_err() {
                    // Caller dropped the reply receiver — skip this message
                    return Ok(());
                }
                (ContinuationDto::Reply, Some(bin_tx))
            }
            Continuation::Forward(tx) => {
                // Forward needs async into_dto(). Spawn a one-off task (rare path).
                let mailbox = self.mailbox.clone();
                crate::compat::spawn(async move {
                    let k = Continuation::Forward(tx);
                    let Some(dto) = k.into_dto().await else {
                        return;
                    };
                    let frame_dto: MsgPackDto<A> = (msg, dto);
                    let bytes = match postcard::to_stdvec(&frame_dto) {
                        Ok(b) => b,
                        Err(_) => return,
                    };
                    mailbox.buffer.lock().unwrap().push(PendingFrame { bytes, reply_tx: None });
                    let _ = mailbox.work_tx.send(mailbox.actor_id);
                });
                return Ok(());
            }
            // BinReply/LocalBinForward/RemoteBinForward shouldn't appear in user-facing send()
            _ => (ContinuationDto::Nil, None),
        };

        let dto: MsgPackDto<A> = (msg, k_dto);
        let bytes = match postcard::to_stdvec(&dto) {
            Ok(b) => b,
            Err(err) => {
                error!(%err, "failed to serialize message for remote import");
                return Ok(()); // Drop silently (matches current behavior)
            }
        };

        self.mailbox.buffer.lock().unwrap().push(PendingFrame { bytes, reply_tx });
        let _ = self.mailbox.work_tx.send(self.mailbox.actor_id);
        Ok(())
    }
}

#[derive(Debug)]
struct ImportPool {
    work_tx: theta_flume::Sender<ActorId>,
    states: Arc<ConcurrentMap<ActorId, Arc<ImportMailbox>>>,
}

impl ImportPool {
    fn new(num_workers: usize) -> Self {
        let (work_tx, work_rx) = theta_flume::unbounded_anonymous();
        let states: Arc<ConcurrentMap<ActorId, Arc<ImportMailbox>>> =
            Arc::new(ConcurrentMap::default());

        for i in 0..num_workers {
            let work_rx = work_rx.clone();
            let states = states.clone();
            crate::compat::spawn(async move {
                trace!(worker = i, "import flush worker started");
                Self::flush_worker(work_rx, states).await;
                trace!(worker = i, "import flush worker stopped");
            });
        }

        ImportPool { work_tx, states }
    }

    fn register(&self, actor_id: ActorId, mailbox: Arc<ImportMailbox>) {
        self.states.insert(actor_id, mailbox);
    }

    fn unregister(&self, actor_id: &ActorId) {
        self.states.remove(actor_id);
    }

    async fn flush_worker(
        work_rx: theta_flume::Receiver<ActorId>,
        states: Arc<ConcurrentMap<ActorId, Arc<ImportMailbox>>>,
    ) {
        use futures::stream::{FuturesUnordered, StreamExt};
        let mut in_flight = FuturesUnordered::new();

        loop {
            if in_flight.is_empty() {
                // Nothing in-flight: block on work queue
                let Some(actor_id) = work_rx.recv().await else { break };
                if let Some(state) = states.with(&actor_id, |s| s.clone()) {
                    in_flight.push(Self::drain_actor(state));
                }
            } else {
                // Has in-flight: poll both work queue and in-flight drains
                futures::select_biased! {
                    _ = in_flight.select_next_some() => {
                        // drain completed, loop to check more
                    }
                    mb_actor_id = work_rx.recv().fuse() => {
                        let Some(actor_id) = mb_actor_id else { break };
                        if let Some(state) = states.with(&actor_id, |s| s.clone()) {
                            in_flight.push(Self::drain_actor(state));
                        }
                    }
                }
            }
        }
    }

    async fn drain_actor(state: Arc<ImportMailbox>) {
        // Drain buffer atomically
        let pending: Vec<PendingFrame> = {
            let mut buf = state.buffer.lock().unwrap();
            if buf.is_empty() {
                return;
            }
            std::mem::take(&mut *buf)
        };

        let mut stream = state.stream.lock().await;

        // Initialize stream if needed (lazy open)
        if matches!(*stream, StreamState::NotReady) {
            match state.conn.open_bi().await {
                Ok((mut send_half, recv_half)) => {
                    #[cfg(feature = "perf-instrument")]
                    crate::perf_instrument::OPEN_BI_COUNT
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                    if let Err(err) = send_half.send_frame(state.init_bytes.clone()).await {
                        error!(
                            actor_id = %Hex(state.actor_id.as_bytes()),
                            %err,
                            "failed to send import init frame",
                        );
                        *stream = StreamState::Failed;
                        state.closed.store(true, Ordering::Release);
                        return;
                    }
                    #[cfg(feature = "perf-instrument")]
                    crate::perf_instrument::IMPORT_SETUP_OK
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                    *stream = StreamState::Ready(send_half, recv_half);
                }
                Err(err) => {
                    error!(
                        actor_id = %Hex(state.actor_id.as_bytes()),
                        %err,
                        "failed to open bi-stream for import",
                    );
                    #[cfg(feature = "perf-instrument")]
                    crate::perf_instrument::OPEN_BI_FAIL_COUNT
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    *stream = StreamState::Failed;
                    state.closed.store(true, Ordering::Release);
                    return;
                }
            }
        }

        let StreamState::Ready(ref mut send_half, ref mut recv_half) = *stream else {
            // Failed state — drop pending frames
            return;
        };

        // Batch send all frames, then batch read replies
        let mut reply_txs: Vec<oneshot::Sender<(Peer, Vec<u8>)>> = Vec::new();

        for frame in pending {
            if let Err(err) = send_half.send_frame(frame.bytes).await {
                warn!(
                    actor_id = %Hex(state.actor_id.as_bytes()),
                    %err,
                    "failed to send frame, closing import",
                );
                state.closed.store(true, Ordering::Release);
                *stream = StreamState::Failed;
                return;
            }

            #[cfg(feature = "perf-instrument")]
            crate::perf_instrument::IMPORT_COUNT
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            if let Some(tx) = frame.reply_tx {
                reply_txs.push(tx);
            }
        }

        // Read replies in FIFO order (protocol guarantees ordering)
        for tx in reply_txs {
            let mut buf = Vec::new();
            if let Err(err) = recv_half.recv_frame_into(&mut buf).await {
                warn!(
                    actor_id = %Hex(state.actor_id.as_bytes()),
                    %err,
                    "reply read failed, closing import",
                );
                state.closed.store(true, Ordering::Release);
                *stream = StreamState::Failed;
                return;
            }

            #[cfg(feature = "perf-instrument")]
            crate::perf_instrument::REPLY_COUNT
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            let _ = tx.send((state.peer.clone(), buf));
        }
    }
}

// Implementations

impl LocalPeer {
    #[allow(dead_code)]
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
            Entry::Vacant(v) => {
                let conn = self.0.network.connect_and_prepare(public_key);

                let peer = Peer::new(public_key, conn);

                trace!(%peer, "adding");
                v.insert(peer.clone());

                peer
            }
            Entry::Occupied(o) => o.get().clone(),
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
        self.0.import_pool.unregister(actor_id);
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
        crate::compat::timeout(
            Duration::from_secs(5),
            recv_half.read_exact(&mut status),
        )
        .await
        .map_err(|_| RemoteError::Timeout)?
        .map_err(|e| NetworkError::ReadExactError(Arc::new(e)))?;

        if status[0] == 0x00 {
            let mut buf = Vec::new();
            recv_half
                .recv_frame_into(&mut buf)
                .await?;
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
        crate::compat::timeout(
            Duration::from_secs(5),
            recv_half.recv_frame_into(&mut buf),
        )
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
                        Ok(bi) => bi,
                        Err(err) => {
                            break warn!(from = %self, %err, "failed to accept bi-stream");
                        }
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
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    return warn!(from = %peer, %err, "failed to receive initial frame on bi-stream");
                }
                Err(_) => {
                    return warn!(from = %peer, "timeout receiving initial frame on bi-stream, dropping dead stream");
                }
            }

            let init_frame: InitFrame = match postcard::from_bytes(&buf) {
                Err(err) => {
                    return error!(from = %peer, %err, "failed to deserialize initial frame");
                }
                Ok(frame) => frame,
            };

            match init_frame {
                InitFrame::Import { actor_id } => {
                    let Ok(actor) = RootContext::lookup_any_local_unchecked_impl(
                        actor_id.as_bytes(),
                    ) else {
                        return error!(from = %peer, actor_id = %Hex(actor_id.as_bytes()), "local actor to export not found");
                    };

                    if let Err(err) =
                        actor.spawn_export_task(peer.clone(), in_stream, reply_tx)
                    {
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
                    let err_bytes = match postcard::to_stdvec(&MonitorError::from(BindingError::NotFound)) {
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
        let uuid = actor_id; // ActorId is Uuid

        let init_frame = InitFrame::Import { actor_id };
        let init_bytes = match postcard::to_stdvec(&init_frame) {
            Err(err) => {
                error!(
                    actor = format_args!("{}({actor_id})", type_name::<A>()),
                    host = %self,
                    %err,
                    "failed to serialize import initial frame",
                );
                // Return a "dead" remote sender with closed=true
                let (work_tx, _) = theta_flume::unbounded_anonymous();
                let mailbox = Arc::new(ImportMailbox {
                    actor_id,
                    uuid,
                    peer: self.clone(),
                    conn: self.0.conn.clone(),
                    init_bytes: Vec::new(),
                    buffer: std::sync::Mutex::new(Vec::new()),
                    stream: tokio::sync::Mutex::new(StreamState::Failed),
                    work_tx,
                    closed: AtomicBool::new(true),
                    sender_count: AtomicUsize::new(1),
                });
                return ActorRef::from_remote(RemoteSender {
                    mailbox,
                    _phantom: PhantomData,
                });
            }
            Ok(bytes) => bytes,
        };

        let pool = &LocalPeer::inst().0.import_pool;

        let mailbox = Arc::new(ImportMailbox {
            actor_id,
            uuid,
            peer: self.clone(),
            conn: self.0.conn.clone(),
            init_bytes,
            buffer: std::sync::Mutex::new(Vec::new()),
            stream: tokio::sync::Mutex::new(StreamState::NotReady),
            work_tx: pool.work_tx.clone(),
            closed: AtomicBool::new(false),
            sender_count: AtomicUsize::new(1),
        });

        pool.register(actor_id, mailbox.clone());

        trace!(
            actor = format_args!("{}({actor_id})", type_name::<A>()),
            host = %self,
            "registered imported remote (task-eliminated)",
        );

        ActorRef::from_remote(RemoteSender {
            mailbox,
            _phantom: PhantomData,
        })
    }

    /// No need of task_local PEER
    async fn send_control_frame(&self, frame: &ControlFrame) -> Result<(), RemoteError> {
        #[cfg(feature = "perf-instrument")]
        let t_total = std::time::Instant::now();

        #[cfg(feature = "perf-instrument")]
        let t_ser = std::time::Instant::now();
        let bytes = postcard::to_stdvec(frame).map_err(RemoteError::SerializeError)?;
        #[cfg(feature = "perf-instrument")]
        crate::perf_instrument::CTRL_SERIALIZE_NS.fetch_add(
            t_ser.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        self.0.conn.send_control_frame(&bytes).await?;

        #[cfg(feature = "perf-instrument")]
        {
            crate::perf_instrument::CTRL_SEND_COUNT
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            crate::perf_instrument::CTRL_TOTAL_NS.fetch_add(
                t_total.elapsed().as_nanos() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
        }

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
        let num_workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);

        Self {
            public_key: network.public_key(),
            network,
            peers: ConcurrentMap::default(),
            imports: ConcurrentMap::default(),
            import_pool: ImportPool::new(num_workers),
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
