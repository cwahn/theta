use std::{
    any::Any,
    cell::RefCell,
    path::PathBuf,
    sync::{Arc, LazyLock, RwLock},
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
use tokio::{sync::mpsc, task_local};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::{
    actor::{Actor, ActorId},
    actor_ref::{MsgPack, MsgRx, MsgTx},
    base::PROJECT_DIRS,
    message::{Continuation, DynMessage},
    prelude::ActorRef,
    remote::REGISTRY,
};

const ALPN: &[u8] = b"theta";

task_local! {
    static MB_TARGET_PEER: RefCell<Option<Arc<RemotePeer>>>;
    static MB_SENDER_PEER: RefCell<Option<Arc<RemotePeer>>>;
}

static LOCAL_PEER: LazyLock<LocalPeer> = LazyLock::new(|| {
    tokio::runtime::Runtime::new()
        .expect("Failed to create tokio runtime")
        .block_on(LocalPeer::init(
            get_or_init_public_key().expect("Failed to get or create public key"),
        ))
        .expect("Failed to initialize local peer")
});

type AnyMsgTx = Box<dyn Any + Send + Sync>; // type-erased MsgTx<A: Actor> 
type AnyMsgRx = Box<dyn Any + Send + Sync>; // type-erased MsgRx<A: Actor>

type ReplyKey = Uuid;

type HashMap<K, V> = FxHashMap<K, V>;
type HashSet<T> = FxHashSet<T>;

struct LocalPeer {
    public_key: PublicKey,
    endpoint: iroh::endpoint::Endpoint,
    remote_peers: RwLock<HashMap<PublicKey, Arc<RemotePeer>>>,

    imports: RwLock<HashMap<ActorId, Import>>,
}

#[derive(Debug)]
struct RemotePeer {
    conn: iroh::endpoint::Connection,
    public_key: PublicKey,
    exports: RwLock<HashSet<ActorId>>,
    pending_exports: RwLock<HashMap<ActorId, oneshot::Sender<iroh::endpoint::RecvStream>>>,
    pending_reply_recvs: RwLock<HashMap<ReplyKey, oneshot::Sender<Vec<u8>>>>,
}

struct Import {
    host_peer: PublicKey,
    tx: AnyMsgTx,
}

// Regular serde will be enough
#[derive(Serialize, Deserialize)]
enum PeerDatagram {
    Actor(ActorId),           // Subsequent uni stream is for this actor
    Reply(ReplyKey, Vec<u8>), // Reply message
}

fn get_or_init_public_key() -> Option<PublicKey> {
    let key_pair_path = PROJECT_DIRS.config_dir().join("keypair/");
    let private_key_path = key_pair_path.join("id_ed25519");
    let public_key_path = key_pair_path.join("id_ed25519.pub");

    get_key(&private_key_path, &public_key_path)
        .or_else(|| create_key_pair(&private_key_path, &public_key_path))
}

fn get_key(private_key_path: &PathBuf, public_key_path: &PathBuf) -> Option<PublicKey> {
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

    Some(public_key)
}

fn create_key_pair(private_key_path: &PathBuf, public_key_path: &PathBuf) -> Option<PublicKey> {
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

    Some(public_key)
}

impl LocalPeer {
    async fn init(public_key: PublicKey) -> anyhow::Result<Self> {
        let endpoint = iroh::endpoint::Endpoint::builder()
            .alpns(vec![ALPN.to_vec()])
            .discovery_n0()
            .bind()
            .await?;

        tokio::spawn(Self::run_endpoint(endpoint.clone()));

        Ok(Self {
            public_key,
            endpoint,
            remote_peers: RwLock::new(HashMap::default()),

            imports: RwLock::new(HashMap::default()),
        })
    }

    fn get() -> &'static Self {
        &LOCAL_PEER
    }

    async fn run_endpoint(endpoint: iroh::endpoint::Endpoint) {
        let remote_ctx = LocalPeer::get();

        while let Some(incomming) = endpoint.accept().await {
            match incomming.await {
                Ok(conn) => {
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

    async fn get_or_init_peer(&self, public_key: PublicKey) -> anyhow::Result<Arc<RemotePeer>> {
        if let Some(peer) = self.remote_peers.read().unwrap().get(&public_key) {
            return Ok(peer.clone());
        }

        self.connect_to_peer(public_key).await
    }

    fn get_peer(&self, public_key: &PublicKey) -> Option<Arc<RemotePeer>> {
        self.remote_peers.read().unwrap().get(public_key).cloned()
    }

    async fn connect_to_peer(&self, public_key: PublicKey) -> anyhow::Result<Arc<RemotePeer>> {
        let addr: NodeAddr = public_key.into();

        let Some(conn) = self.endpoint.connect(addr, ALPN).await.ok() else {
            return Err(anyhow!("Failed to connect to peer"));
        };

        info!("Connected to remote peer: {public_key}");

        let remote_peer = RemotePeer::init(conn);

        self.remote_peers
            .write()
            .unwrap()
            .insert(public_key, remote_peer.clone());

        Ok(remote_peer)
    }

    async fn handle_inbound_conn(&self, conn: iroh::endpoint::Connection) -> anyhow::Result<()> {
        let public_key = conn.remote_node_id()?;

        let peer = Arc::new(RemotePeer::new(conn));

        self.remote_peers.write().unwrap().insert(public_key, peer);

        Ok(())
    }

    fn get_imported_host_public_key(&self, actor_id: &ActorId) -> Option<PublicKey> {
        self.imports
            .read()
            .unwrap()
            .get(actor_id)
            .map(|import| import.host_peer.clone())
    }

    fn get_imported<A: Actor>(&self, actor_id: ActorId) -> Option<ActorRef<A>> {
        let tx = self
            .imports
            .read()
            .unwrap()
            .get(&actor_id)
            .and_then(|import| import.tx.downcast_ref::<MsgTx<A>>().cloned())?;

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
                    error!("Failed to get or init peer: {e}");
                    return;
                }
            };

            let mut out_stream = match peer.request_out_stream(actor_id.clone()).await {
                Ok(stream) => stream,
                Err(e) => {
                    error!("Failed to request out stream: {e}");
                    return;
                }
            };

            while let Some(msg_k) = rx.recv().await {
                MB_TARGET_PEER.with(|p| {
                    p.replace(Some(peer.clone()));
                });

                let msg_k_dto: MsgPackDto<A> = msg_k.into();

                let bytes = match postcard::to_stdvec(&msg_k_dto) {
                    Ok(data) => data,
                    Err(e) => {
                        error!("Failed to serialize message: {e}");
                        break;
                    }
                };

                MB_TARGET_PEER.with(|p| {
                    p.replace(None);
                });

                // todo Better messaging protocol
                let size = bytes.len() as u64;

                if let Err(e) = out_stream.write_all(&size.to_le_bytes()).await {
                    error!("Failed to write size to stream: {e}");
                    break;
                }

                if let Err(e) = out_stream.write_all(&bytes).await {
                    error!("Failed to write data to stream: {e}");
                    break;
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
        let inst = Arc::new(Self::new(conn));

        tokio::spawn({
            let inst = inst.clone();
            async move {
                if let Err(e) = inst.run().await {
                    error!("Remote peer connection error: {e}");
                }
            }
        });

        inst
    }

    fn new(conn: iroh::endpoint::Connection) -> Self {
        let public_key = conn.remote_node_id().unwrap();

        Self {
            conn,
            public_key,
            exports: RwLock::new(HashSet::default()),
            pending_exports: RwLock::new(HashMap::default()),
            pending_reply_recvs: RwLock::new(HashMap::default()),
        }
    }

    async fn run(&self) -> anyhow::Result<()> {
        while let Ok(bytes) = self.conn.read_datagram().await {
            let Ok(datagram) = postcard::from_bytes::<PeerDatagram>(&bytes) else {
                error!("Failed to deserialize datagram");
                continue;
            };

            match datagram {
                PeerDatagram::Actor(actor_id) => {
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
                PeerDatagram::Reply(reply_key, data) => {
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

        info!(
            "Remote peer connection closed: {}",
            self.conn.remote_node_id()?
        );

        LocalPeer::get()
            .remote_peers
            .write()
            .unwrap()
            .remove(&self.conn.remote_node_id()?);

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
        let (tx, rx) = oneshot::channel::<RecvStream>();

        tokio::spawn({
            let actor_tx = actor.tx.clone();
            let self_public_key = self.public_key.clone();

            async move {
                let Ok(mut in_stream) = rx.await else {
                    error!("Failed to receive uni stream for actor {}", actor.id);
                    return;
                };

                let Some(sender_peer) = LOCAL_PEER.get_peer(&self_public_key) else {
                    error!("Failed to get sender peer for actor {}", actor.id);
                    return;
                };

                // todo Better messaging protocol

                let mut size_buf = [0; 8];
                if let Err(e) = in_stream.read_exact(&mut size_buf).await {
                    error!("Failed to read size from stream: {e}");
                    return;
                }

                let size = u64::from_le_bytes(size_buf);

                let mut data = vec![0; size as usize];
                if let Err(e) = in_stream.read_exact(&mut data).await {
                    error!("Failed to read data from stream: {e}");
                    return;
                }

                MB_SENDER_PEER.with(|p| {
                    p.replace(Some(sender_peer));
                });

                let msg_k_dto = match postcard::from_bytes::<MsgPackDto<A>>(&data) {
                    Ok(msg_k_dto) => msg_k_dto,
                    Err(e) => {
                        error!("Failed to deserialize message: {e}");
                        return;
                    }
                };

                let msg_k = msg_k_dto.into();

                MB_SENDER_PEER.with(|p| {
                    p.replace(None);
                });

                if let Err(e) = actor_tx.send(msg_k) {
                    error!("Failed to send message to actor {}: {e}", actor.id);
                    return;
                };
            }
        });

        self.pending_exports.write().unwrap().insert(actor.id, tx);

        Ok(())
    }

    fn arrange_recv_reply(&self, reply_tx: oneshot::Sender<Vec<u8>>) -> anyhow::Result<ReplyKey> {
        let reply_key = Uuid::new_v4();

        self.pending_reply_recvs
            .write()
            .unwrap()
            .insert(reply_key, reply_tx);

        Ok(reply_key)
    }

    async fn send_reply<A: Actor>(
        &self,
        reply_key: ReplyKey,
        reply_bytes: Vec<u8>,
    ) -> anyhow::Result<()> {
        let bytes = postcard::to_stdvec(&PeerDatagram::Reply(reply_key, reply_bytes))?;

        self.conn
            .send_datagram(bytes.into())
            .map_err(|e| anyhow!("Failed to send reply datagram: {e}"))?;

        Ok(())
    }

    async fn request_out_stream(
        &self,
        actor_id: ActorId,
    ) -> anyhow::Result<iroh::endpoint::SendStream> {
        let bytes = bytes::Bytes::from(postcard::to_stdvec(&PeerDatagram::Actor(actor_id))?);

        self.conn
            .send_datagram(bytes)
            .map_err(|e| anyhow!("Failed to send actor request datagram: {e}"))?;

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
        let public_key = match LOCAL_PEER.get_imported_host_public_key(&self.id) {
            Some(public_key) => public_key,
            None => {
                let Some(target_peer) = MB_TARGET_PEER.with(|p| p.borrow().clone()) else {
                    return Err(serde::ser::Error::custom("No target peer set"));
                };

                if !target_peer.is_exported(&self.id) {
                    if let Err(e) = target_peer.arrange_export(self.clone()) {
                        return Err(serde::ser::Error::custom(format!(
                            "Failed to arrange export for actor {}: {e}",
                            self.id
                        )));
                    }
                }

                LOCAL_PEER.public_key.clone()
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
        let (public_key, actor_id): (PublicKey, ActorId) = Deserialize::deserialize(deserializer)?;

        match LOCAL_PEER.get_imported(actor_id) {
            Some(actor_ref) => Ok(actor_ref),
            None => {
                let (tx, rx) = mpsc::unbounded_channel();

                LOCAL_PEER.arrange_import(public_key, actor_id, tx.clone(), rx);

                Ok(ActorRef { id: actor_id, tx })
            }
        }
    }
}

#[derive(Debug)]
struct MsgPackDto<A>
where
    A: Actor,
{
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
                            let Some(target_peer) = MB_TARGET_PEER.with(|p| p.borrow().clone())
                            else {
                                error!("Failed to get target peer");

                                return ContinurationDto::Reply(ReplyKey::nil());
                            };

                            let reply_key = target_peer
                                .arrange_recv_reply(reply_bytes_tx)
                                .unwrap_or_else(|e| {
                                    error!("Failed to arrange reply key: {e}, returning ReplyKey::nil()");

                                    ReplyKey::nil()
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
        let k = match self.k_dto {
            ContinurationDto::Reply(reply_key) => {
                if reply_key.is_nil() {
                    Continuation::nil()
                } else {
                    let actor_impl_id = A::__IMPL_ID;
                    let behavior_impl_id = self.msg.__impl_id();

                    let Some(serialize_fn) = REGISTRY
                        .read()
                        .unwrap()
                        .get(&actor_impl_id)
                        .and_then(|any_registry| {
                            any_registry.downcast_ref::<::theta::remote::serde::MsgRegistry<A>>()
                        })
                        .and_then(|registry| registry.0.get(&behavior_impl_id))
                        .map(|entry| entry.serialize_return_fn)
                    else {
                        error!(
                            "Failed to get serialize function for actor_impl_id: {actor_impl_id}, behavior_impl_id: {behavior_impl_id}",
                        );
                        return (self.msg, Continuation::nil());
                    };

                    let Some(sender_peer) = MB_SENDER_PEER.with(|s| s.borrow().clone()) else {
                        error!("Failed to get sender peer");
                        return (self.msg, Continuation::nil());
                    };

                    let (reply_tx, reply_rx) = oneshot::channel();

                    tokio::spawn(async move {
                        let Ok(any_ret) = reply_rx.await else {
                            error!("Failed to get reply for key {reply_key}");
                            return;
                        };

                        let Ok(bytes) = serialize_fn(&any_ret) else {
                            error!("Failed to serialize reply for key {reply_key}");
                            return;
                        };

                        if let Err(e) = sender_peer.send_reply::<A>(reply_key, bytes).await {
                            error!("Failed to send reply: {e}");
                        }
                    });

                    Continuation::reply(reply_tx)
                }
            } // todo ContinurationDto::Forward(actor_id) => Continuation::forward(actor_id, OneShot::new()),
        };

        (self.msg, k)
    }
}
