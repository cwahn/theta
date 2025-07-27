use std::{
    any::Any,
    cell::RefCell,
    sync::{Arc, LazyLock, RwLock},
};

use anyhow::anyhow;
use futures::{channel::oneshot, future::BoxFuture};
use iroh::{NodeAddr, PublicKey};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
    ser::SerializeStruct,
};
use tokio::{sync::mpsc, task_local};
use tracing::{error, info};
use uuid::Uuid;

use crate::{
    actor::{Actor, ActorId},
    actor_ref::{MsgPack, MsgRx, MsgTx},
    message::{Continuation, DynMessage, OneShot},
    prelude::ActorRef,
    remote::REGISTRY,
};

// todo Remove Any

const ALPN: &[u8] = b"theta";

task_local! {
    // static MB_SENDER: Arc<RemotePeer>;
    static MB_TARGET_PEER: RefCell<Option<Arc<RemotePeer>>>;
    // static MB_TARGET: RefCell<Option<PublicKey>>;
    static MB_SENDER_PEER: RefCell<Option<Arc<RemotePeer>>>;
}

type AnyMsgTx = Box<dyn Any + Send + Sync>; // type-erased MsgTx<A: Actor> 
type AnyMsgRx = Box<dyn Any + Send + Sync>; // type-erased MsgRx<A: Actor>

type AnyMsgPack = (Box<dyn Any + Send + Sync>, Continuation); // type-erased MsgPack<A: Actor>

type ActorTypeId = Uuid;
type MsgTypeId = Uuid;
type ReplyKey = Uuid;

type HashMap<K, V> = FxHashMap<K, V>;
type HashSet<T> = FxHashSet<T>;

static LOCAL_PEER: LazyLock<LocalPeer> = LazyLock::new(|| {
    tokio::runtime::Runtime::new()
        .expect("Failed to create tokio runtime")
        .block_on(LocalPeer::init(get_or_init_public_key()))
        .expect("Failed to initialize local peer")
});

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

fn get_or_init_public_key() -> PublicKey {
    todo!("Load public key from storage or create one and store it")
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
                        #[cfg(feature = "tracing")]
                        error!("Failed to handle new connection: {}", e);
                    }
                }
                Err(e) => {
                    #[cfg(feature = "tracing")]
                    error!("Failed to accept connection: {}", e);
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

        #[cfg(feature = "tracing")]
        info!("Connected to remote peer: {}", public_key);

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

    // fn is_imported(&self, actor_id: &ActorId) -> bool {
    //     self.imports.read().unwrap().contains_key(actor_id)
    // }

    fn get_host_public_key(&self, actor_id: &ActorId) -> PublicKey {
        match self.imports.read().unwrap().get(actor_id) {
            Some(import) => import.host_peer.clone(),
            None => self.public_key.clone(), // Local actor
        }
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
                    #[cfg(feature = "tracing")]
                    error!("Failed to get or init peer: {}", e);
                    return;
                }
            };

            let mut out_stream = match peer.request_out_stream(actor_id.clone()).await {
                Ok(stream) => stream,
                Err(e) => {
                    #[cfg(feature = "tracing")]
                    error!("Failed to request out stream: {}", e);
                    return;
                }
            };

            while let Some(msg_k) = rx.recv().await {
                // todo set target context to public_key
                // At this point, continuation must be converted as serializable form.
                // Actually it is quite impossible to serialize the original continuation,
                // - nil is no problem
                // - reply should be relayed
                // - forward does contain information regarding the type of actor.
                // It has to be sent to the remote peer so that it can convert it back to continuation.

                let msg_k_dto: MsgPackDto<A> = msg_k.into();

                if let Err(e) = send_msg_pack(&mut out_stream, &msg_k_dto).await {
                    #[cfg(feature = "tracing")]
                    error!("Failed to send message: {}", e);
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

    fn arrange_forward_tx(&self, actor_id: ActorId) -> anyhow::Result<OneShot> {
        todo!("Should return immediately")
    }
}

impl RemotePeer {
    fn init(conn: iroh::endpoint::Connection) -> Arc<Self> {
        let inst = Arc::new(Self::new(conn));

        tokio::spawn({
            let inst = inst.clone();
            async move {
                if let Err(e) = inst.run().await {
                    #[cfg(feature = "tracing")]
                    error!("Remote peer connection error: {}", e);
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
                #[cfg(feature = "tracing")]
                error!("Failed to deserialize datagram");
                continue;
            };

            match datagram {
                PeerDatagram::Actor(actor_id) => {
                    // Expect a incoming uni stream for this actor
                    let Some(tx) = self.pending_exports.write().unwrap().remove(&actor_id) else {
                        #[cfg(feature = "tracing")]
                        tracing::warn!("No pending export for actor {}", actor_id);
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
                        #[cfg(feature = "tracing")]
                        tracing::warn!("No pending reply for key {}", reply_key);
                        continue;
                    };

                    tx.send(data)
                        .map_err(|_| anyhow!("Failed to send reply data"))?;
                }
            }
        }

        #[cfg(feature = "tracing")]
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

    // fn public_key(&self) -> PublicKey {
    //     self.conn.remote_node_id().unwrap()
    // }

    fn is_exported(&self, actor_id: &ActorId) -> bool {
        self.exports.read().unwrap().contains(actor_id)
    }

    async fn arrange_export<A>(&self, actor: ActorRef<A>) -> anyhow::Result<()>
    where
        A: Actor,
        DynMessage<A>: Serialize + for<'d> Deserialize<'d>,
    {
        let (tx, rx) = oneshot::channel();

        tokio::spawn({
            let actor_tx = actor.tx.clone();
            let self_public_key = self.public_key.clone();

            async move {
                // let in_stream = rx.await?;

                let Ok(mut in_stream) = rx.await else {
                    #[cfg(feature = "tracing")]
                    error!("Failed to receive uni stream for actor {}", actor.id);
                    return;
                };

                // todo Set source remote peer context to self.

                while let Ok(msg_k_dto) = recv_msg_pack::<MsgPackDto<A>>(&mut in_stream).await {
                    let Some(sender_peer) = LOCAL_PEER.get_peer(&self_public_key) else {
                        #[cfg(feature = "tracing")]
                        error!("Failed to get sender peer for actor {}", actor.id);
                        return;
                    };

                    MB_SENDER_PEER.with(|p| {
                        p.replace(Some(sender_peer));
                    });

                    let msg_k = msg_k_dto.into();

                    MB_SENDER_PEER.with(|p| {
                        p.replace(None);
                    });

                    if let Err(e) = actor_tx.send(msg_k) {
                        #[cfg(feature = "tracing")]
                        error!("Failed to send message to actor {}: {}", actor.id, e);
                        break;
                    }
                }
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
            .map_err(|e| anyhow!("Failed to send reply datagram: {}", e))?;

        Ok(())
    }

    async fn request_out_stream(
        &self,
        actor_id: ActorId,
    ) -> anyhow::Result<iroh::endpoint::SendStream> {
        let bytes = bytes::Bytes::from(postcard::to_stdvec(&PeerDatagram::Actor(actor_id))?);

        self.conn
            .send_datagram(bytes)
            .map_err(|e| anyhow!("Failed to send actor request datagram: {}", e))?;

        self.conn
            .open_uni()
            .await
            .map_err(|e| anyhow!("Failed to open uni stream for actor {}: {}", actor_id, e))
    }
}

// Deserialization & Import - out_stream

// struct SerializedActorRef<A: Actor> {
//     public_key: PublicKey,
//     actor_id: ActorId,
// }

impl<A> Serialize for ActorRef<A>
where
    A: Actor,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let public_key = LOCAL_PEER.get_host_public_key(&self.id);

        let actor_id = self.id;

        (public_key, actor_id).serialize(serializer)
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

// Reply will be encoded into a reply key
// Forward will be encoded into ActorId and handled like regular import with relaying one shot

// pub trait ForwardTarget: erased_serde::Serialize {
//     fn to_oneshot(&self) -> OneShot;
// }

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
                            #[cfg(feature = "tracing")]
                            error!("Failed to send reply bytes tx");

                            ContinurationDto::Reply(ReplyKey::nil())
                        }
                        Ok(_) => {
                            let Some(target_peer) = MB_TARGET_PEER.with(|p| p.borrow().clone())
                            else {
                                #[cfg(feature = "tracing")]
                                error!("Failed to get target peer");

                                return ContinurationDto::Reply(ReplyKey::nil());
                            };

                            let reply_key = target_peer
                                .arrange_recv_reply(reply_bytes_tx)
                                .unwrap_or_else(|e| {
                                    #[cfg(feature = "tracing")]
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
                        #[cfg(feature = "tracing")]
                        error!(
                            "Failed to get serialize function for actor_impl_id: {actor_impl_id}, behavior_impl_id: {behavior_impl_id}",
                        );

                        return (self.msg, Continuation::nil());
                    };

                    let Some(sender_peer) = MB_SENDER_PEER.with(|s| s.borrow().clone()) else {
                        #[cfg(feature = "tracing")]
                        error!("Failed to get sender peer");

                        return (self.msg, Continuation::nil());
                    };

                    let (reply_tx, reply_rx) = oneshot::channel();

                    tokio::spawn(async move {
                        let Ok(any_ret) = reply_rx.await else {
                            #[cfg(feature = "tracing")]
                            error!("Failed to get reply for key {}", reply_key);
                            return;
                        };

                        let Ok(bytes) = serialize_fn(&any_ret) else {
                            #[cfg(feature = "tracing")]
                            error!("Failed to serialize reply for key {}", reply_key);
                            return;
                        };

                        if let Err(e) = sender_peer.send_reply::<A>(reply_key, bytes).await {
                            #[cfg(feature = "tracing")]
                            error!("Failed to send reply: {}", e);
                        }
                    });

                    Continuation::reply(reply_tx)
                }
            } // todo ContinurationDto::Forward(actor_id) => Continuation::forward(actor_id, OneShot::new()),
        };

        (self.msg, k)
    }
}

async fn send_msg_pack<T: Serialize>(
    stream: &mut iroh::endpoint::SendStream,
    obj: &T,
) -> anyhow::Result<()> {
    // todo Should be encoded with target info
    let data = postcard::to_stdvec(obj)?;
    let size = data.len() as u64;

    stream.write_all(&size.to_le_bytes()).await?;
    stream.write_all(&data).await?;

    Ok(())
}

async fn recv_msg_pack<T: for<'de> Deserialize<'de>>(
    stream: &mut iroh::endpoint::RecvStream,
) -> anyhow::Result<T> {
    let mut size_buf = [0; 8];
    stream.read_exact(&mut size_buf).await?;
    let size = u64::from_le_bytes(size_buf);

    let mut data = vec![0; size as usize];
    stream.read_exact(&mut data).await?;
    // todo Decoded with source info
    let msg_k = postcard::from_bytes(&data)?;

    Ok(msg_k)
}

// Should import on deserialization
// And should arrange oneshot for forwarding

// #[derive(Serialize, Deserialize)]
// enum Ft {
//     Pre(Box<dyn ForwardTarget>),
//     Post(OneShot),
// }

// // Each remote actor should be able designate it self as a forward target
// // So each actor type should apply to the register
// // Which is not needed at the moment.
// trait ForwardTarget: Send + Sync {
//     // fn arrange_send_forward(&self) -> BoxFuture<'_, Result<(), Box<dyn Any + Send>>>;
// }

// #[derive(Debug)]
// struct SomeActor;

// impl Actor for SomeActor {}

// impl ForwardTarget for ActorRef<SomeActor> {}
