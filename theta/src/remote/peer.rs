use std::{
    any::Any,
    borrow::Cow,
    path::PathBuf,
    sync::{Arc, RwLock},
    vec,
};

use anyhow::anyhow;
use futures::channel::oneshot;
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
use tracing::{debug, error, info, trace, warn};
use uuid::Uuid;

use crate::{
    actor::{Actor, ActorId},
    actor_ref::{MsgPack, MsgRx, MsgTx},
    base::PROJECT_DIRS,
    global_context::ActorBindings,
    message::{Continuation, DynMessage},
    prelude::ActorRef,
    remote::{ACTOR_REGISTRY, registry::MsgRegistry, serde::ActorImplId},
};

const ALPN: &[u8] = b"theta";

// todo Maybe better option?

static LOCAL_PEER: OnceCell<LocalPeer> = OnceCell::const_new();

task_local! {
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
    pending_reply_recvs: RwLock<HashMap<ReplyKey, oneshot::Sender<Vec<u8>>>>,

    pending_lookup_reqs: RwLock<HashMap<PeerReqId, oneshot::Sender<Option<Vec<u8>>>>>,
}

#[derive(Debug)]
struct Import {
    host_peer: PublicKey,
    tx: AnyMsgTx,
}

// Regular serde will be enough
#[derive(Serialize, Deserialize)]
enum PeerMsg {
    LookupReq {
        ident: Cow<'static, str>,
        actor_impl_id: ActorImplId,
        peer_req_id: PeerReqId,
    },
    LookupRes {
        actor_bytes: Option<Vec<u8>>,
        peer_req_id: PeerReqId,
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
        warn!("Failed to create keypair directory: {}", e);
        return None;
    }

    if let Err(e) = std::fs::write(private_key_path, secret_key.to_bytes()) {
        warn!("Failed to write private key: {}", e);
        return None;
    }

    if let Err(e) = std::fs::write(public_key_path, public_key.as_bytes()) {
        warn!("Failed to write public key: {}", e);
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

    pub(crate) async fn lookup<A: Actor>(
        &self,
        public_key: PublicKey,
        ident: String,
    ) -> anyhow::Result<Option<ActorRef<A>>>
    where
        DynMessage<A>: Serialize + for<'d> Deserialize<'d>,
    {
        let peer = self.get_or_init_peer(public_key).await?;

        REMOTE_PEER
            .scope(peer.clone(), peer.lookup::<A>(ident))
            .await
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
        tx: MsgTx<A>,
        mut rx: MsgRx<A>,
    ) where
        A: Actor,
        DynMessage<A>: Serialize + for<'d> Deserialize<'d>,
    {
        tokio::spawn(async move {
            self.register_import(public_key, actor_id, tx);

            let peer = match self.get_or_init_peer(public_key.clone()).await {
                Ok(peer) => peer,
                Err(e) => {
                    return error!("Failed to get or init peer: {e}");
                }
            };

            let mut out_stream = match peer.request_out_stream(actor_id.clone()).await {
                Ok(stream) => stream,
                Err(e) => {
                    return error!("Failed to request out stream: {e}");
                }
            };

            // while let Some(msg_k) = rx.recv().await {
            loop {
                let Some(msg_k) = rx.recv().await else {
                    break warn!("Message receiver closed for actor {actor_id}");
                };

                let bytes = match REMOTE_PEER.sync_scope(peer.clone(), || {
                    let msg_k_dto: MsgPackDto<A> = msg_k.into();

                    postcard::to_stdvec(&msg_k_dto)
                        .map_err(|e| anyhow!("Failed to serialize message: {e}"))
                }) {
                    Ok(data) => data,
                    Err(e) => {
                        break error!("Failed to serialize message: {e}");
                    }
                };

                // todo Better messaging protocol
                let size = bytes.len() as u64;

                if let Err(e) = out_stream.write_all(&size.to_le_bytes()).await {
                    break error!("Failed to write size to stream: {e}");
                }

                if let Err(e) = out_stream.write_all(&bytes).await {
                    break error!("Failed to write data to stream: {e}");
                }
            }

            self.unregister_import(&actor_id)
        });
    }

    fn register_import<A: Actor>(&self, public_key: PublicKey, actor_id: ActorId, tx: MsgTx<A>) {
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
            let inst = peer.clone();

            async move {
                if let Err(e) = REMOTE_PEER.scope(inst.clone(), inst.run()).await {
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
            pending_reply_recvs: RwLock::new(HashMap::default()),
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

                    let uni_stream = self.conn.accept_uni().await?;

                    tx.send(uni_stream)
                        .map_err(|_| anyhow!("Failed to send uni stream"))?;

                    self.exports.write().unwrap().insert(actor_id);
                }
                PeerMsg::Reply(reply_key, data) => {
                    debug!("Received reply for key: {reply_key}");
                    // Handle reply message
                    let Some(tx) = self.pending_reply_recvs.write().unwrap().remove(&reply_key)
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
        A: Actor,
        DynMessage<A>: Serialize + for<'d> Deserialize<'d>,
    {
        trace!("Arranging export for actor: {}", actor.id);
        let (tx, rx) = oneshot::channel::<RecvStream>();

        let remote_peer = REMOTE_PEER.with(|p| p.clone());

        tokio::spawn({
            let actor_tx = actor.tx.clone();

            REMOTE_PEER.scope(remote_peer, async move {
                let Ok(mut in_stream) = rx.await else {
                    return error!("Failed to receive uni stream for actor {}", actor.id);
                };

                // todo Better messaging protocol
                let mut size_buf = [0; 8];
                loop {
                    if let Err(e) = in_stream.read_exact(&mut size_buf).await {
                        break error!("Failed to read size from stream: {e}");
                    }

                    let size = u64::from_le_bytes(size_buf);

                    let mut data = vec![0; size as usize];

                    if let Err(e) = in_stream.read_exact(&mut data).await {
                        break error!("Failed to read data from stream: {e}");
                    }

                    let msg_k_dto = match postcard::from_bytes::<MsgPackDto<A>>(&data) {
                        Ok(msg_k_dto) => msg_k_dto,
                        Err(e) => {
                            error!("Failed to deserialize message: {e}");
                            continue;
                        }
                    };

                    let msg_k = msg_k_dto.into();

                    if let Err(e) = actor_tx.send(msg_k) {
                        break error!("Failed to send message to actor {}: {e}", actor.id);
                    }
                }

                warn!("Incoming stream for actor {} closed", actor.id);
            })
        });

        self.pending_exports.write().unwrap().insert(actor.id, tx);

        Ok(())
    }

    fn arrange_recv_reply(&self, reply_tx: oneshot::Sender<Vec<u8>>) -> anyhow::Result<ReplyKey> {
        trace!("Arranging reply for pending request");
        let reply_key = Uuid::new_v4();

        self.pending_reply_recvs
            .write()
            .unwrap()
            .insert(reply_key, reply_tx);

        Ok(reply_key)
    }

    async fn lookup<A: Actor>(&self, ident: String) -> anyhow::Result<Option<ActorRef<A>>>
    where
        DynMessage<A>: Serialize + for<'d> Deserialize<'d>,
    {
        trace!("Looking up actor by ident: {ident}");

        let peer_req_id = Uuid::new_v4();
        let msg = PeerMsg::LookupReq {
            ident: ident.into(),
            actor_impl_id: A::__IMPL_ID,
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

    async fn send_reply<A: Actor>(
        &self,
        reply_key: ReplyKey,
        reply_bytes: Vec<u8>,
    ) -> anyhow::Result<()> {
        let bytes = postcard::to_stdvec(&PeerMsg::Reply(reply_key, reply_bytes))?;

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
        let bytes = bytes::Bytes::from(postcard::to_stdvec(&PeerMsg::Actor(actor_id))?);

        debug!("Notifying remote peer to open uni stream for actor: {actor_id}");
        self.conn
            .send_datagram(bytes)
            .map_err(|e| anyhow!("Failed to send actor request datagram: {e}"))?;

        debug!("Opening uni stream for actor: {actor_id}");
        self.conn
            .open_uni()
            .await
            .map_err(|e| anyhow!("Failed to open uni stream for actor {actor_id}: {e}"))
    }
}

// Deserialization & Import - out_stream

impl<A> Serialize for ActorRef<A>
where
    A: Actor,
    DynMessage<A>: Serialize + for<'d> Deserialize<'d>,
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
    A: Actor,
    DynMessage<A>: Serialize + for<'d> Deserialize<'d>,
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

                Ok(ActorRef { id: actor_id, tx })
            }
        }
    }
}

#[derive(Debug)]
struct MsgPackDto<A: Actor> {
    pub msg: DynMessage<A>,
    pub k_dto: ContinurationDto,
}

impl<A> Serialize for MsgPackDto<A>
where
    A: Actor,
    DynMessage<A>: Serialize + for<'d> Deserialize<'d>,
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
    A: Actor,
    DynMessage<A>: Serialize + for<'d> Deserialize<'d>,
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
            A: Actor,
            DynMessage<A>: Serialize + for<'d> Deserialize<'d>,
        {
            marker: std::marker::PhantomData<MsgPackDto<A>>,
            lifetime: std::marker::PhantomData<&'de ()>,
        }

        impl<'de, A> Visitor<'de> for _Visitor<'de, A>
        where
            A: Actor,
            DynMessage<A>: Serialize + for<'d> Deserialize<'d>,
        {
            type Value = MsgPackDto<A>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct MsgPackDto")
            }

            fn visit_seq<S>(self, mut seq: S) -> Result<Self::Value, S::Error>
            where
                S: de::SeqAccess<'de>,
            {
                let msg = seq.next_element::<DynMessage<A>>()?.ok_or_else(|| {
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
                let mut msg: Option<DynMessage<A>> = None;
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

impl<A> From<MsgPack<A>> for MsgPackDto<A>
where
    A: Actor,
    DynMessage<A>: Serialize + for<'d> Deserialize<'d>,
{
    fn from(value: MsgPack<A>) -> Self {
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
            }, // todo Continuation::Forward(actor_id, _) => EncodedContinuation::Forward(*actor_id),
        }
    }
}

impl<A> Into<MsgPack<A>> for MsgPackDto<A>
where
    A: Actor,
    DynMessage<A>: Serialize + for<'d> Deserialize<'d>,
{
    fn into(self) -> MsgPack<A> {
        trace!("Converting MsgPackDto to MsgPack");
        let k = match self.k_dto {
            ContinurationDto::Reply(reply_key) => {
                trace!("Converting ContinurationDto::Reply to Continuation::Reply");
                if reply_key.is_nil() {
                    Continuation::nil()
                } else {
                    let actor_impl_id = A::__IMPL_ID;
                    let behavior_impl_id = self.msg.__impl_id();

                    let Some(serialize_fn) = ACTOR_REGISTRY
                        .get(&actor_impl_id)
                        .and_then(|e| e.msg_registry.downcast_ref::<MsgRegistry<A>>())
                        .and_then(|r| r.0.get(&behavior_impl_id))
                        .map(|e| e.serialize_return_fn)
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
