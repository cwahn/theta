use std::{
    any::Any,
    borrow::Cow,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
    vec,
};

use anyhow::anyhow;
use futures::{SinkExt, StreamExt, channel::oneshot};
use iroh::{NodeAddr, PublicKey, SecretKey, endpoint::RecvStream};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
    ser::SerializeStruct,
};
use tokio::{
    sync::{OnceCell, mpsc},
    task_local,
};
use tokio_util::codec::{FramedRead, FramedWrite};
use tracing::{debug, error, info, trace, warn};
use uuid::Uuid;

use crate::{
    actor::{Actor, ActorId},
    base::PROJECT_DIRS,
    global_context::ActorBindings,
    message::{BoxedMsg, Continuation, Message, MsgPack, MsgRx, MsgTx},
    monitor::ReportTx,
    prelude::ActorRef,
    remote::{
        ACTOR_REGISTRY, BoxedRemoteMsg, Remote, RemoteActor, RemoteMsgPack, RemoteMsgRx,
        RemoteMsgTx, codec::PostcardCobsCodec, registry::BehaviorRegistry,
    },
};

const ALPN: &[u8] = b"theta";

// todo Make local peer to be non-static so that destructor could be called
static LOCAL_PEER: OnceCell<LocalPeer> = OnceCell::const_new();

task_local! {
    // todo This should be a trait object(s) supporting import and export
    static REMOTE_PEER: Arc<RemotePeer>;
}

type AnyMsgTx = Box<dyn Any + Send + Sync>; // type-erased MsgTx<A: Actor> 
// type AnyMsgRx = Box<dyn Any + Send + Sync>; // type-erased MsgRx<A: Actor>

type PeerReqId = Uuid;
type ReplyKey = Uuid;

type HashMap<K, V> = FxHashMap<K, V>;
type HashSet<T> = FxHashSet<T>;

#[derive(Debug)]
pub(crate) struct LocalPeer {
    pub(crate) public_key: PublicKey,
    endpoint: iroh::endpoint::Endpoint,
    remote_peers: RwLock<HashMap<PublicKey, Arc<RemotePeer>>>,

    imports: RwLock<HashMap<ActorId, Import>>,

    bindings: ActorBindings,
}

#[derive(Debug)]
pub(crate) struct RemotePeer {
    conn: iroh::endpoint::Connection,
    public_key: PublicKey,
    exports: RwLock<HashSet<ActorId>>,
    pending_exports: RwLock<HashMap<ActorId, oneshot::Sender<iroh::endpoint::RecvStream>>>,
    pending_reply_recvs: Mutex<HashMap<ReplyKey, oneshot::Sender<Vec<u8>>>>,

    pending_lookup_reqs: RwLock<HashMap<PeerReqId, oneshot::Sender<Option<Vec<u8>>>>>,
}

#[derive(Debug)]
struct Import {
    host_peer: PublicKey,
    tx: AnyMsgTx,
}

// Regular serde will be enough
// todo Restructure this to follow actor's message
#[derive(Serialize, Deserialize)]
enum PeerMsg {
    LookupReq {
        actor_impl_id: ActorImplId,
        ident: Cow<'static, str>,
        peer_req_id: PeerReqId,
    },
    LookupRes {
        actor_bytes: Option<Vec<u8>>,
        peer_req_id: PeerReqId,
    },
    ObserveReq {
        actor_impl_id: ActorImplId,
        actor_id: ActorId,
        peer_req_id: PeerReqId,
    },
    ObserveRes {
        peer_req_id: PeerReqId, // There should be unistream to
    },
    Actor(ActorId),           // Expect a incoming uni stream for this actor
    Reply(ReplyKey, Vec<u8>), // Reply message
}

fn get_or_init_public_key() -> Option<SecretKey> {
    let key_pair_path = PROJECT_DIRS.config_dir().join("keypair/");
    let private_key_path = key_pair_path.join("id_ed25519");
    let public_key_path = key_pair_path.join("id_ed25519.pub");

    get_key(&private_key_path, &public_key_path)
        .or_else(|| create_key_pair(&private_key_path, &public_key_path))
}

fn get_key(private_key_path: &PathBuf, public_key_path: &PathBuf) -> Option<SecretKey> {
    let private_key_bytes = match std::fs::read(private_key_path) {
        Ok(bytes) => bytes,
        Err(e) => {
            warn!("Private key file not found at {private_key_path:?}: {e}");
            return None;
        }
    };

    let private_key_slice = match private_key_bytes.as_slice().try_into() {
        Ok(slice) => slice,
        Err(e) => {
            warn!("Invalid private key file at {private_key_path:?}: {e}");
            return None;
        }
    };

    let public_key_bytes = match std::fs::read(public_key_path) {
        Ok(bytes) => bytes,
        Err(e) => {
            warn!("Public key file not found at {public_key_path:?}: {e}");
            return None;
        }
    };

    let public_key_slice = match public_key_bytes.as_slice().try_into() {
        Ok(slice) => slice,
        Err(e) => {
            warn!("Invalid public key file at {public_key_path:?}: {e}");
            return None;
        }
    };

    let private_key = SecretKey::from_bytes(private_key_slice);
    let public_key = match PublicKey::from_bytes(public_key_slice) {
        Ok(key) => key,
        Err(e) => {
            warn!("Failed to parse public key from bytes: {e}");
            return None;
        }
    };

    if private_key.public() != public_key {
        warn!("Public key does not match private key");
        return None;
    }

    Some(private_key)
}

fn create_key_pair(private_key_path: &PathBuf, public_key_path: &PathBuf) -> Option<SecretKey> {
    let secret_key = SecretKey::generate(rand::rngs::OsRng);
    let public_key = secret_key.public();

    if let Err(e) = std::fs::create_dir_all(private_key_path.parent().unwrap()) {
        warn!("Failed to create keypair directory: {e}");
        return None;
    }

    if let Err(e) = std::fs::write(private_key_path, secret_key.to_bytes()) {
        warn!("Failed to write private key: {e}");
        return None;
    }

    if let Err(e) = std::fs::write(public_key_path, public_key.as_bytes()) {
        warn!("Failed to write public key: {e}");
        return None;
    }

    Some(secret_key)
}

impl LocalPeer {
    pub(crate) async fn initialize(bindings: ActorBindings) -> &'static Self {
        let private_key = get_or_init_public_key().expect("Failed to get or create public key");

        LOCAL_PEER
            .get_or_init(
                || async move { LocalPeer::init_impl(private_key, bindings).await.unwrap() },
            )
            .await
    }

    pub(crate) fn get() -> &'static Self {
        LOCAL_PEER.get().expect("LocalPeer not initialized")
    }

    pub(crate) async fn lookup<A: RemoteActor>(
        &self,
        public_key: PublicKey,
        ident: String,
    ) -> anyhow::Result<Option<ActorRef<A>>> {
        let peer = self.get_or_init_peer(public_key).await?;

        REMOTE_PEER
            .scope(peer.clone(), peer.lookup::<A>(ident))
            .await
    }

    // todo observe_local

    pub(crate) async fn observe<A: RemoteActor>(
        &self,
        actor_id: ActorId,
        tx: ReportTx<A>,
    ) -> anyhow::Result<()> {
        let Some(pk) = self.get_imported_host_public_key(&actor_id) else {
            return Err(anyhow!("Failed to get public key for actor: {actor_id}"));
        };

        let Some(peer) = self.get_peer(&pk) else {
            return Err(anyhow!("Failed to get peer for public key: {pk}"));
        };

        peer.observe(actor_id, tx)
    }

    async fn init_impl(private_key: SecretKey, bindings: ActorBindings) -> anyhow::Result<Self> {
        let endpoint = iroh::endpoint::Endpoint::builder()
            .secret_key(private_key)
            .alpns(vec![ALPN.to_vec()])
            .discovery_n0()
            .bind()
            .await?;

        tokio::spawn(Self::run_endpoint(endpoint.clone()));

        Ok(Self {
            public_key: endpoint.node_id(),
            endpoint,
            remote_peers: RwLock::new(HashMap::default()),

            imports: RwLock::new(HashMap::default()),

            bindings,
        })
    }

    async fn run_endpoint(endpoint: iroh::endpoint::Endpoint) {
        let remote_ctx = LocalPeer::get();

        let some = b"abc";

        while let Some(incomming) = endpoint.accept().await {
            match incomming.await {
                Ok(conn) => {
                    debug!(
                        "Accepted new connection from remote peer: {:?}",
                        conn.remote_node_id()
                    );

                    if let Err(e) = remote_ctx.handle_inbound_conn(conn).await {
                        error!("Failed to handle new connection: {e}");
                    }
                }
                Err(e) => {
                    error!("Failed to accept connection: {e}");
                }
            }
        }
    }

    async fn connect_peer(&self, public_key: PublicKey) -> anyhow::Result<Arc<RemotePeer>> {
        trace!("Connecting to remote peer: {public_key}");
        let addr: NodeAddr = public_key.into();

        let Some(conn) = self.endpoint.connect(addr, ALPN).await.ok() else {
            return Err(anyhow!("Failed to connect to peer"));
        };

        debug!("Connected to remote peer: {public_key}");
        let remote_peer = RemotePeer::init(conn);

        self.remote_peers
            .write()
            .unwrap()
            .insert(public_key, remote_peer.clone());

        Ok(remote_peer)
    }

    async fn get_or_init_peer(&self, public_key: PublicKey) -> anyhow::Result<Arc<RemotePeer>> {
        match self.get_peer(&public_key) {
            Some(peer) => Ok(peer),
            None => self.connect_peer(public_key).await,
        }
    }

    fn get_peer(&self, public_key: &PublicKey) -> Option<Arc<RemotePeer>> {
        self.remote_peers.read().unwrap().get(public_key).cloned()
    }

    async fn handle_inbound_conn(&self, conn: iroh::endpoint::Connection) -> anyhow::Result<()> {
        let public_key = conn.remote_node_id()?;

        let peer = RemotePeer::init(conn);

        self.remote_peers.write().unwrap().insert(public_key, peer);

        Ok(())
    }

    fn get_imported_host_public_key(&self, actor_id: &ActorId) -> Option<PublicKey> {
        self.imports
            .read()
            .unwrap()
            .get(actor_id)
            .map(|i| i.host_peer.clone())
    }

    fn get_imported<A: Actor>(&self, actor_id: ActorId) -> Option<ActorRef<A>> {
        let tx = self
            .imports
            .read()
            .unwrap()
            .get(&actor_id)
            .and_then(|i| i.tx.downcast_ref::<MsgTx<A>>().cloned())?;

        Some(ActorRef {
            id: actor_id,
            tx: tx.clone(),
        })
    }

    fn arrange_import<A>(
        &'static self,
        public_key: PublicKey,
        actor_id: ActorId,
        tx: RemoteMsgTx<A>,
        mut rx: RemoteMsgRx<A>,
    ) where
        A: RemoteActor,
    {
        tokio::spawn(async move {
            self.register_import(public_key, actor_id, tx);

            let peer = match self.get_or_init_peer(public_key.clone()).await {
                Ok(peer) => peer,
                Err(e) => {
                    return error!("Failed to get or init peer: {e}");
                }
            };

            let out_stream = match peer.request_out_stream(actor_id.clone()).await {
                Ok(stream) => stream,
                Err(e) => {
                    return error!("Failed to request out stream: {e}");
                }
            };

            let mut framed_writer =
                FramedWrite::new(out_stream, PostcardCobsCodec::<MsgPackDto<A>>::default());

            loop {
                let Some(msg_k) = rx.recv().await else {
                    break warn!("Message receiver closed for actor {actor_id}");
                };

                let msg_k_dto = match REMOTE_PEER.sync_scope(peer.clone(), || {
                    let msg_k_dto: MsgPackDto<A> = msg_k.into();
                    Ok::<MsgPackDto<A>, anyhow::Error>(msg_k_dto)
                }) {
                    Ok(dto) => dto,
                    Err(e) => {
                        break error!("Failed to convert message: {e}");
                    }
                };

                if let Err(e) = framed_writer.send(msg_k_dto).await {
                    break error!("Failed to send message: {e}");
                }
            }

            self.unregister_import(&actor_id);
        });
    }

    fn register_import<A: Actor>(
        &self,
        public_key: PublicKey,
        actor_id: ActorId,
        tx: RemoteMsgTx<A>,
    ) {
        let import = Import {
            tx: Box::new(tx),
            host_peer: public_key,
        };

        self.imports.write().unwrap().insert(actor_id, import);
    }

    fn unregister_import(&self, actor_id: &ActorId) {
        self.imports.write().unwrap().remove(actor_id);
    }

    // todo fn arrange_forward_tx(&self, actor_id: ActorId) -> anyhow::Result<OneShot> {
    //     todo!("Should return immediately")
    // }
}

impl RemotePeer {
    fn init(conn: iroh::endpoint::Connection) -> Arc<Self> {
        let peer = Arc::new(Self::new(conn));

        tokio::spawn({
            let peer = peer.clone();

            async move {
                if let Err(e) = REMOTE_PEER.scope(peer.clone(), peer.run()).await {
                    error!("Remote peer connection error: {e}");
                }
            }
        });

        peer
    }

    fn new(conn: iroh::endpoint::Connection) -> Self {
        let public_key = conn.remote_node_id().expect("Failed to get remote node ID");

        Self {
            conn,
            public_key,
            exports: RwLock::new(HashSet::default()),
            pending_exports: RwLock::new(HashMap::default()),
            pending_reply_recvs: Mutex::new(HashMap::default()),
            pending_lookup_reqs: RwLock::new(HashMap::default()),
        }
    }

    async fn run(&self) -> anyhow::Result<()> {
        trace!("Running remote peer connection: {}", self.public_key);

        while let Ok(bytes) = self.conn.read_datagram().await {
            debug!("Received datagram from remote peer: {}", self.public_key);

            let Ok(datagram) = postcard::from_bytes::<PeerMsg>(&bytes) else {
                error!("Failed to deserialize datagram");
                continue;
            };

            match datagram {
                PeerMsg::LookupReq {
                    ident,
                    actor_impl_id,
                    peer_req_id,
                } => {
                    debug!("Received lookup request for ident: {ident}");

                    let res = match LocalPeer::get().bindings.read().unwrap().get(&ident) {
                        Some(any_actor) => {
                            match ACTOR_REGISTRY.get(&actor_impl_id).map(|e| {
                                let actor_bytes = (e.serialize_fn)(any_actor);

                                actor_bytes
                            }) {
                                Some(Ok(bytes)) => PeerMsg::LookupRes {
                                    actor_bytes: Some(bytes),
                                    peer_req_id,
                                },
                                Some(Err(e)) => {
                                    error!("Failed to serialize actor for ident: {ident}: {e}");
                                    PeerMsg::LookupRes {
                                        actor_bytes: None,
                                        peer_req_id,
                                    }
                                }
                                None => {
                                    error!(
                                        "No serialize function found for actor impl id: {actor_impl_id}"
                                    );
                                    PeerMsg::LookupRes {
                                        actor_bytes: None,
                                        peer_req_id,
                                    }
                                }
                            }
                        }
                        None => {
                            warn!("Actor not found for ident: {ident}");

                            PeerMsg::LookupRes {
                                actor_bytes: None,
                                peer_req_id,
                            }
                        }
                    };

                    let bytes = postcard::to_stdvec(&res)
                        .map_err(|e| anyhow!("Failed to serialize lookup response: {e}"))?;

                    if let Err(e) = self.conn.send_datagram(bytes.into()) {
                        warn!("Failed to send lookup response: {e}");
                    }
                }
                PeerMsg::LookupRes {
                    actor_bytes,
                    peer_req_id,
                } => {
                    debug!("Received lookup response for peer req id: {peer_req_id}");

                    let Some(tx) = self
                        .pending_lookup_reqs
                        .write()
                        .unwrap()
                        .remove(&peer_req_id)
                    else {
                        error!("No pending request for peer req id: {peer_req_id}");
                        continue;
                    };

                    if let Err(_e) = tx.send(actor_bytes) {
                        error!("Failed to send lookup response");
                    }
                }
                PeerMsg::Actor(actor_id) => {
                    debug!("Received actor export request for actor id: {actor_id}");
                    // Expect a incoming uni stream for this actor
                    let Some(tx) = self.pending_exports.write().unwrap().remove(&actor_id) else {
                        tracing::warn!("No pending export for actor {actor_id}");
                        continue;
                    };

                    // ! This might be a wrong and random stream
                    let uni_stream = self.conn.accept_uni().await?;

                    tx.send(uni_stream)
                        .map_err(|_| anyhow!("Failed to send uni stream"))?;

                    self.exports.write().unwrap().insert(actor_id);
                }
                PeerMsg::Reply(reply_key, data) => {
                    debug!("Received reply for key: {reply_key}");
                    // Handle reply message
                    let Some(tx) = self.pending_reply_recvs.lock().unwrap().remove(&reply_key)
                    else {
                        tracing::warn!("No pending reply for key {reply_key}");
                        continue;
                    };

                    tx.send(data)
                        .map_err(|_| anyhow!("Failed to send reply data"))?;
                }
            }
        }

        info!("Remote peer connection closed: {}", self.public_key);

        LocalPeer::get()
            .remote_peers
            .write()
            .unwrap()
            .remove(&self.public_key);

        Ok(())
    }

    fn is_exported(&self, actor_id: &ActorId) -> bool {
        self.exports.read().unwrap().contains(actor_id)
    }

    fn arrange_export<A>(&self, actor: ActorRef<A>) -> anyhow::Result<()>
    where
        A: RemoteActor,
    {
        trace!("Arranging export for actor: {}", actor.id);

        let (tx, rx) = oneshot::channel::<RecvStream>();
        let remote_peer = REMOTE_PEER.with(|p| p.clone());

        tokio::spawn({
            let actor_tx = actor.tx.clone();
            let actor_id = actor.id;

            REMOTE_PEER.scope(remote_peer, async move {
                let in_stream = match rx.await {
                    Ok(stream) => stream,
                    Err(_) => {
                        return error!("Failed to receive uni stream for actor {actor_id}");
                    }
                };

                let mut framed_reader =
                    FramedRead::new(in_stream, PostcardCobsCodec::<MsgPackDto<A>>::default());

                while let Some(result) = framed_reader.next().await {
                    let msg_k_dto = match result {
                        Ok(dto) => dto,
                        Err(e) => {
                            error!("Failed to deserialize message: {e}");
                            continue;
                        }
                    };

                    let msg_k = msg_k_dto.into();

                    if let Err(e) = actor_tx.send(msg_k) {
                        error!("Failed to send message to actor {actor_id}: {e}");
                        break;
                    }
                }

                warn!("Incoming stream for actor {actor_id} closed");
            })
        });

        self.pending_exports.write().unwrap().insert(actor.id, tx);
        Ok(())
    }

    fn arrange_recv_reply(&self, reply_tx: oneshot::Sender<Vec<u8>>) -> anyhow::Result<ReplyKey> {
        trace!("Arranging reply for pending request");
        let reply_key = Uuid::new_v4();

        self.pending_reply_recvs
            .lock()
            .unwrap()
            .insert(reply_key, reply_tx);

        Ok(reply_key)
    }

    async fn lookup<A: RemoteActor>(&self, ident: String) -> anyhow::Result<Option<ActorRef<A>>> {
        trace!("Looking up actor by ident: {ident}");

        let peer_req_id = Uuid::new_v4();
        let msg = PeerMsg::LookupReq {
            ident: ident.into(),
            // actor_impl_id: A::__IMPL_ID,
            actor_impl_id: A::TID,
            peer_req_id,
        };

        let bytes = postcard::to_stdvec(&msg)
            .map_err(|e| anyhow!("Failed to serialize lookup request: {e}"))?;

        let (tx, rx) = oneshot::channel();
        self.pending_lookup_reqs
            .write()
            .unwrap()
            .insert(peer_req_id, tx);

        debug!("Sending lookup request for the ident peer req id: {peer_req_id}");
        self.conn
            .send_datagram(bytes.into())
            .map_err(|e| anyhow!("Failed to send lookup request: {e}"))?;

        match rx.await {
            Ok(actor_bytes) => {
                if let Some(bytes) = actor_bytes {
                    debug!("Received Some lookup response for ident");

                    let actor_ref: ActorRef<A> = postcard::from_bytes(&bytes)
                        .map_err(|e| anyhow!("Failed to deserialize actor bytes: {e}"))?;

                    Ok(Some(actor_ref))
                } else {
                    debug!("Received None lookup response for the ident");

                    Ok(None)
                }
            }
            Err(_) => Err(anyhow!("Failed to receive lookup response")),
        }
    }

    // In order to keep local monitoring zero-cost, the monitoring channel is properly typed.
    // Which means it can not use Box<dyn Any> serializing as I do for forwarding.
    // It should be mor like a message channel.
    // But that works because it it took trait object.
    // What I need is data. not trait object.
    // No I need some different way.
    // But also, it has to keep

    async fn observe<A: RemoteActor>(
        &self,
        actor_id: ActorId,
        report_tx: ReportTx<A>,
    ) -> anyhow::Result<()> {
        let peer_req_id = Uuid::new_v4();
        let msg = PeerMsg::ObserveReq {
            actor_impl_id: A::TID,
            actor_id,
            peer_req_id,
        };

        let bytes = postcard::to_stdvec(&msg)
            .map_err(|e| anyhow!("Failed to serialize observe request: {e}"))?;

        let (tx, rx) = oneshot::channel();
        self.pending_lookup_reqs
            .write()
            .unwrap()
            .insert(peer_req_id, tx);

        debug!("Sending observe request for actor: {actor_id}, peer req id: {peer_req_id}");
        self.conn
            .send_datagram(bytes.into())
            .map_err(|e| anyhow!("Failed to send observe request: {e}"))?;

        match rx.await {
            Ok(_) => {
                debug!("Received observe response for actor: {actor_id}");
                todo!();
            }
            Err(e) => Err(anyhow!("Failed to receive observe response: {e}")),
        }
    }

    async fn send_reply<A: Actor>(
        &self,
        reply_key: ReplyKey,
        bytes: Vec<u8>,
    ) -> anyhow::Result<()> {
        let bytes = postcard::to_stdvec(&PeerMsg::Reply(reply_key, bytes))?;

        debug!("Sending reply for key: {reply_key}");
        self.conn
            .send_datagram(bytes.into())
            .map_err(|e| anyhow!("Failed to send reply datagram: {e}"))?;

        Ok(())
    }

    async fn request_out_stream(
        &self,
        actor_id: ActorId,
    ) -> anyhow::Result<iroh::endpoint::SendStream> {
        let bytes = postcard::to_stdvec(&PeerMsg::Actor(actor_id))?;

        debug!("Notifying remote peer to open uni stream for actor: {actor_id}");
        self.conn
            .send_datagram(bytes.into())
            .map_err(|e| anyhow!("Failed to send actor request datagram: {e}"))?;

        debug!("Opening uni stream for actor: {actor_id}");
        self.conn
            .open_uni()
            .await
            .map_err(|e| anyhow!("Failed to open uni stream for actor {actor_id}: {e}"))
    }
}

// Deserialization & Import - out_stream
// todo Change internal format of ActorRef to use Url so that it could support other protocols

impl<A> Serialize for ActorRef<A>
where
    A: RemoteActor,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        trace!("Serializing ActorRef for actor: {}", self.id);

        let public_key = match LocalPeer::get().get_imported_host_public_key(&self.id) {
            Some(public_key) => public_key,
            None => {
                trace!("Actor {} is not imported, which means local", self.id);

                let _ = REMOTE_PEER.with(|p| {
                    if !p.is_exported(&self.id) {
                        trace!("Arranging export for actor: {}", self.id);

                        if let Err(e) = p.arrange_export(self.clone()) {
                            return Err(serde::ser::Error::custom(format!(
                                "Failed to arrange export for actor {}: {e}",
                                self.id
                            )));
                        }

                        trace!("Export arranged for actor: {}", self.id);
                    }
                    Ok(())
                })?;

                LocalPeer::get().public_key.clone()
            }
        };

        (public_key, self.id).serialize(serializer)
    }
}

impl<'de, A> Deserialize<'de> for ActorRef<A>
where
    A: RemoteActor,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        trace!("Deserializing ActorRef for actor");

        let (public_key, actor_id): (PublicKey, ActorId) = Deserialize::deserialize(deserializer)?;

        match LocalPeer::get().get_imported(actor_id) {
            Some(actor_ref) => Ok(actor_ref),
            None => {
                trace!("Actor {actor_id} is not imported, arranging import");

                let (tx, rx) = mpsc::unbounded_channel();

                LocalPeer::get().arrange_import(public_key, actor_id, tx.clone(), rx);

                // ! THIS IS a problem
                // ! It makes difference that what could be put into the tx.
                // ! Which means it is not the same type of Actor Ref.
                // ! Which means in order to make Actor as remote, all of it's message should be remote

                Ok(ActorRef { id: actor_id, tx })
            }
        }
    }
}

#[derive(Debug)]
struct MsgPackDto<A: RemoteActor> {
    pub msg: BoxedRemoteMsg<A>,
    pub k_dto: ContinurationDto,
}

impl<A> Serialize for MsgPackDto<A>
where
    A: RemoteActor,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("MsgPackDto", 2)?;
        state.serialize_field("msg", &self.msg)?;
        state.serialize_field("k_dto", &self.k_dto)?;
        state.end()
    }
}

impl<'de, A> Deserialize<'de> for MsgPackDto<A>
where
    A: RemoteActor,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        enum Field {
            Msg,
            KDto,
            Ignore,
        }

        struct FieldVisitor;

        impl<'de> Visitor<'de> for FieldVisitor {
            type Value = Field;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("field identifier")
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match v {
                    0 => Ok(Field::Msg),
                    1 => Ok(Field::KDto),
                    _ => Ok(Field::Ignore),
                }
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match v {
                    "msg" => Ok(Field::Msg),
                    "k_dto" => Ok(Field::KDto),
                    _ => Ok(Field::Ignore),
                }
            }

            fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match v {
                    b"msg" => Ok(Field::Msg),
                    b"k_dto" => Ok(Field::KDto),
                    _ => Ok(Field::Ignore),
                }
            }
        }

        impl<'de> Deserialize<'de> for Field {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_identifier(FieldVisitor)
            }
        }

        struct _Visitor<'de, A>
        where
            A: RemoteActor,
            BoxedRemoteMsg<A>: Serialize + for<'d> Deserialize<'d>,
        {
            marker: std::marker::PhantomData<MsgPackDto<A>>,
            lifetime: std::marker::PhantomData<&'de ()>,
        }

        impl<'de, A> Visitor<'de> for _Visitor<'de, A>
        where
            A: RemoteActor,
        {
            type Value = MsgPackDto<A>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct MsgPackDto")
            }

            fn visit_seq<S>(self, mut seq: S) -> Result<Self::Value, S::Error>
            where
                S: de::SeqAccess<'de>,
            {
                let msg = seq.next_element::<BoxedRemoteMsg<A>>()?.ok_or_else(|| {
                    de::Error::invalid_length(0, &"struct MsgPackDto with 2 elements")
                })?;
                let k_dto = seq.next_element::<ContinurationDto>()?.ok_or_else(|| {
                    de::Error::invalid_length(1, &"struct MsgPackDto with 2 elements")
                })?;

                Ok(MsgPackDto { msg, k_dto })
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: de::MapAccess<'de>,
            {
                let mut msg: Option<BoxedRemoteMsg<A>> = None;
                let mut k_dto: Option<ContinurationDto> = None;

                while let Some(key) = map.next_key::<Field>()? {
                    match key {
                        Field::Msg => {
                            if msg.is_some() {
                                return Err(de::Error::duplicate_field("msg"));
                            }
                            msg = Some(map.next_value()?);
                        }
                        Field::KDto => {
                            if k_dto.is_some() {
                                return Err(de::Error::duplicate_field("k_dto"));
                            }
                            k_dto = Some(map.next_value()?);
                        }
                        Field::Ignore => {
                            let _: de::IgnoredAny = map.next_value()?;
                        }
                    }
                }

                let msg = msg.ok_or_else(|| de::Error::missing_field("msg"))?;
                let k_dto = k_dto.ok_or_else(|| de::Error::missing_field("k_dto"))?;

                Ok(MsgPackDto { msg, k_dto })
            }
        }

        const FIELDS: &'static [&'static str] = &["msg", "k_dto"];

        deserializer.deserialize_struct(
            "MsgPackDto",
            FIELDS,
            _Visitor {
                marker: std::marker::PhantomData::<MsgPackDto<A>>,
                lifetime: std::marker::PhantomData,
            },
        )
    }
}

// Data Transfer Object for Continuation
#[derive(Debug, Serialize, Deserialize)]
enum ContinurationDto {
    Reply(ReplyKey), // ReplyKey is None when no reply is expected
                     // todo Forward(Box<dyn ForwardTarget>),
}

impl<A> From<RemoteMsgPack<A>> for MsgPackDto<A>
where
    A: RemoteActor,
{
    fn from(value: RemoteMsgPack<A>) -> Self {
        MsgPackDto {
            msg: value.0,
            k_dto: value.1.into(),
        }
    }
}

impl From<Continuation> for ContinurationDto {
    fn from(value: Continuation) -> Self {
        trace!("Converting Continuation to ContinurationDto");
        match value {
            Continuation::Reply(mb_tx) => match mb_tx {
                None => ContinurationDto::Reply(ReplyKey::nil()),
                Some(tx) => {
                    let (reply_bytes_tx, reply_bytes_rx) = oneshot::channel();

                    match tx.send(Box::new(reply_bytes_rx)) {
                        Err(_e) => {
                            error!("Failed to send reply bytes tx");
                            ContinurationDto::Reply(ReplyKey::nil())
                        }
                        Ok(_) => {
                            let reply_key = REMOTE_PEER.with(|p| {
                                p.arrange_recv_reply(reply_bytes_tx)
                                    .unwrap_or_else(|e| {
                                        error!("Failed to arrange reply key: {e}, returning ReplyKey::nil()");
                                        ReplyKey::nil()
                                    })
                            });

                            ContinurationDto::Reply(reply_key)
                        }
                    }
                }
            },
            Continuation::Forward(actor_id, _) => EncodedContinuation::Forward(*actor_id),
        }
    }
}

impl<A> Into<MsgPack<A>> for MsgPackDto<A>
where
    A: RemoteActor,
{
    fn into(self) -> MsgPack<A> {
        trace!("Converting MsgPackDto to MsgPack");
        let k = match self.k_dto {
            ContinurationDto::Reply(reply_key) => {
                trace!("Converting ContinurationDto::Reply to Continuation::Reply");
                if reply_key.is_nil() {
                    Continuation::nil()
                } else {
                    // let actor_impl_id = A::__IMPL_ID;
                    let actor_impl_id = A::TID;
                    let behavior_impl_id = self.msg.tid();

                    let Some(serialize_fn) = ACTOR_REGISTRY
                        .get(&actor_impl_id)
                        .and_then(|e| e.behavior_registry.downcast_ref::<BehaviorRegistry<A>>())
                        .and_then(|r| r.0.get(&behavior_impl_id))
                        .map(|e| e.serialize_return)
                    else {
                        error!(
                            "Failed to get serialize function for actor_impl_id: {actor_impl_id}, behavior_impl_id: {behavior_impl_id}",
                        );
                        return (self.msg, Continuation::nil());
                    };

                    let (reply_tx, reply_rx) = oneshot::channel();

                    trace!("Spawning task to handle reply for key: {reply_key}");
                    let peer = REMOTE_PEER.with(|p| p.clone());
                    tokio::spawn({
                        REMOTE_PEER.scope(peer.clone(), async move {
                            let Ok(any_ret) = reply_rx.await else {
                                return error!("Failed to get reply for key {reply_key}");
                            };

                            // ! Sending return as serialized bytes since recepient knows the exact type.
                            // ! What about serialzie as dyn Any?
                            trace!("Received reply for key: {reply_key}");
                            let Ok(bytes) = serialize_fn(&any_ret) else {
                                return error!("Failed to serialize reply for key {reply_key}");
                            };

                            trace!(
                                "Sending reply for key: {reply_key} with {} bytes",
                                bytes.len()
                            );
                            if let Err(e) = peer.send_reply::<A>(reply_key, bytes).await {
                                error!("Failed to send reply: {e}");
                            }
                        })
                    });

                    Continuation::reply(reply_tx)
                }
            } // todo ContinurationDto::Forward(actor_id) => Continuation::forward(actor_id, OneShot::new()),
        };

        (self.msg, k)
    }
}

// How can I implement forwarding.
// So basically, it could send message to third party actor, not even one in the same machine.
// 1. Send to local actor,
// 2. If it is imported actor, not to take the return and forward but, just render the information of the actor
// 3. Which means the recepient should be able to send message to any actor.
// 4. Once imported, ActorImpl Id, Url, Box<dyn Any> which is one of it's message.
// Downcast to the type of the message since one knows it. Up cast to the dyn message and serialize it from registry.
// So each message implementation should have upcasting function in registry.
// Need to prepare as R -> Box<dyn Message<A>> ->
// R -> Serialize as
// Target actor impl_id.
// impl_id -> (Any -> )

// ActorId
// MessageId

// TypeId for actor and message

// #[type_id("a123b4c5d6e7f8g9h0i1j2k3l4m5n6o7p8q9r")]
// When implementing actor and and message. there should be a macro to register them.
// #[remote] for actor
// #[remote] for actor

// struct ActorEntry {
//     serialize_fn: fn(&Box<dyn Any + Send + Sync>) -> anyhow::Result<Vec<u8>>,
//     behavior_registry: Box<dyn Any + Send + Sync>, // FxHashMap<TypeId, BehaviorEntry<A>>,
// }

// struct BehaviorEntry<A> {
//     deserialize_msg: DeserializeFn<dyn Message<A>>,
//     // Get serialized Box<dyn Message<A>>
//     serialize_msg: FxHashMap<MsgId, fn(&Box<dyn Any + Send + Sync>) -> anyhow::Result<Vec<u8>>>,
//     // Serialize return as is
//     serialize_return_fn: fn(&Box<dyn Any + Send + Sync>) -> anyhow::Result<Vec<u8>>,
// }
