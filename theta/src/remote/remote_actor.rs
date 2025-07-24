use std::{
    any::Any,
    fmt,
    sync::{Arc, LazyLock, RwLock},
};

use futures::channel::oneshot;
use iroh::{NodeAddr, PublicKey};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{self, VariantAccess, Visitor},
};
use tokio::{sync::mpsc, task_local};
use tracing::{error, info};
use uuid::Uuid;

use crate::{
    actor::{Actor, ActorId},
    actor_ref::{MsgPack, MsgRx, MsgTx},
    message::{Continuation, DynMessage, OneShot},
    prelude::ActorRef,
};

// todo Remove Any

const ALPN: &[u8] = b"theta";

task_local! {
    static SENDER: Arc<RemotePeer>;
}

type AnyMsgTx = Box<dyn Any + Send + Sync>; // type-erased MsgTx<A: Actor> 
type AnyMsgRx = Box<dyn Any + Send + Sync>; // type-erased MsgRx<A: Actor>

type AnyMsgPack = (Box<dyn Any + Send + Sync>, Continuation); // type-erased MsgPack<A: Actor>

type ActorTypeId = Uuid;
type MsgTypeId = Uuid;
type ReplyKey = Uuid;

trait RemoteActor: Actor {
    const TYPE_ID: ActorTypeId;
}

trait RemoteMessage {
    const TYPE_ID: MsgTypeId;
}

type HashMap<K, V> = FxHashMap<K, V>;
type HashSet<T> = FxHashSet<T>;

static REMOTE_CTX: LazyLock<LocalPeer> =
    LazyLock::new(|| LocalPeer::init(get_or_init_public_key()));

struct LocalPeer {
    public_key: PublicKey,
    endpoint: iroh::endpoint::Endpoint,
    remote_peers: RwLock<HashMap<PublicKey, Arc<RemotePeer>>>,

    imports: RwLock<HashMap<ActorId, Import>>,
    // ? Do I need this?
    // pending_reply_sends: RwLock<HashMap<ReplyKey, oneshot::Sender<OneShot>>>, ,,,
}

#[derive(Debug)]
struct RemotePeer {
    conn: iroh::endpoint::Connection,
    // exports: Arc<RwLock<HashSet<ActorId>>>,
    exports: RwLock<HashSet<ActorId>>,
    // pending_exports: Arc<RwLock<HashMap<ActorId, oneshot::Sender<iroh::endpoint::RecvStream>>>>,
    pending_exports: RwLock<HashMap<ActorId, oneshot::Sender<iroh::endpoint::RecvStream>>>,
    // pending_reply_recvs: Arc<RwLock<HashMap<ReplyKey, oneshot::Sender<Vec<u8>>>>>,
    pending_reply_recvs: RwLock<HashMap<ReplyKey, oneshot::Sender<Vec<u8>>>>,
}

struct Import {
    host_peer: PublicKey,
    tx: AnyMsgTx,
}

#[derive(Serialize, Deserialize)]
enum Datagram {
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
        &REMOTE_CTX
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
            return Err(anyhow::anyhow!("Failed to connect to peer"));
        };

        #[cfg(feature = "tracing")]
        info!("Connected to remote peer: {}", public_key);

        let remote_peer = Arc::new(RemotePeer::init(conn));

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

    fn is_imported(&self, actor_id: &ActorId) -> bool {
        self.imports.read().unwrap().contains_key(actor_id)
    }

    fn get_imported<A: RemoteActor>(&self, actor_id: ActorId) -> Option<ActorRef<A>> {
        let tx = self.get_imported_tx(&actor_id)?;

        Some(ActorRef {
            id: actor_id,
            tx: tx.clone(),
        })
    }

    fn arrange_import<A: RemoteActor>(
        &'static self,
        public_key: PublicKey,
        actor_id: ActorId,
        tx: MsgTx<A>,
        mut rx: MsgRx<A>,
    ) {
        // ? Which peer should I connect to?
        tokio::spawn(async move {
            self.register_import(actor_id, tx);

            let peer = match self.get_or_init_peer(public_key.clone()).await {
                Ok(peer) => peer,
                Err(e) => {
                    #[cfg(feature = "tracing")]
                    tracing::error!("Failed to get or init peer: {}", e);
                    return;
                }
            };

            let out_stream = match peer.request_out_stream(actor_id.clone()).await {
                Ok(stream) => stream,
                Err(e) => {
                    #[cfg(feature = "tracing")]
                    tracing::error!("Failed to request out stream: {}", e);
                    return;
                }
            };

            while let Some(msg_k) = rx.recv().await {
                if let Err(e) = send_object(&out_stream, &msg_k).await {
                    #[cfg(feature = "tracing")]
                    tracing::error!("Failed to send message: {}", e);
                    break;
                }
            }

            self.unregister_import(&actor_id)
        });
    }

    fn get_imported_tx<A: RemoteActor>(&self, actor_id: &ActorId) -> Option<MsgTx<A>> {
        self.imports
            .read()
            .unwrap()
            .get(actor_id)
            .and_then(|import| import.tx.downcast_ref::<MsgTx<A>>().cloned())
    }

    fn register_import<A: RemoteActor>(&self, actor_id: ActorId, tx: MsgTx<A>) {
        let import = Import {
            tx: Box::new(tx),
            host_peer: None,
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
    fn init(conn: iroh::endpoint::Connection) -> Self {
        let new_peer = Self::new(conn);

        tokio::spawn(new_peer.clone().run());

        new_peer
    }

    fn new(conn: iroh::endpoint::Connection) -> Self {
        Self {
            conn,
            // exports: Arc::new(RwLock::new(HashSet::default())),
            exports: RwLock::new(HashSet::default()),
            // pending_exports: Arc::new(RwLock::new(HashMap::default())),
            pending_exports: RwLock::new(HashMap::default()),
            // pending_reply_recvs: Arc::new(RwLock::new(HashMap::default())),
            pending_reply_recvs: RwLock::new(HashMap::default()),
        }
    }

    async fn run(self) -> anyhow::Result<()> {
        while let Ok(bytes) = self.conn.read_datagram().await {
            let Ok(datagram) = postcard::from_bytes::<Datagram>(&bytes) else {
                #[cfg(feature = "tracing")]
                tracing::error!("Failed to deserialize datagram");
                continue;
            };

            match datagram {
                Datagram::Actor(actor_id) => {
                    // Expect a incoming uni stream for this actor
                    let Some(tx) = self.pending_exports.write().unwrap().remove(&actor_id) else {
                        #[cfg(feature = "tracing")]
                        tracing::warn!("No pending export for actor {}", actor_id);
                        continue;
                    };

                    let uni_stream = self.conn.accept_uni().await?;

                    tx.send(uni_stream)
                        .map_err(|_| anyhow::anyhow!("Failed to send uni stream"))?;

                    self.exports.write().unwrap().insert(actor_id);
                }
                Datagram::Reply(reply_key, data) => {
                    // Handle reply message
                    let Some(tx) = self.pending_reply_recvs.write().unwrap().remove(&reply_key)
                    else {
                        #[cfg(feature = "tracing")]
                        tracing::warn!("No pending reply for key {}", reply_key);
                        continue;
                    };

                    tx.send(data)
                        .map_err(|_| anyhow::anyhow!("Failed to send reply data"))?;
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

    fn is_exported(&self, actor_id: &ActorId) -> bool {
        self.exports.read().unwrap().contains(actor_id)
    }

    async fn arrange_export<A: RemoteActor>(&self, actor: ActorRef<A>) -> anyhow::Result<()> {
        let (tx, rx) = oneshot::channel();

        tokio::spawn({
            let actor_tx = actor.tx.clone();

            async move {
                let in_stream = rx.await?;

                // todo Set remote peer context to self.

                while let msg_k = recv_object::<MsgPack<A>>(&mut in_stream).await? {
                    actor_tx
                        .send(msg_k)
                        .map_err(|_| anyhow::anyhow!("Failed to send message"))?;
                }

                // All remote actor refs get dropped

                Ok(())
            }
        });

        self.pending_exports.write().unwrap().insert(actor.id, tx);

        Ok(())
    }

    async fn send_reply<A: RemoteActor>(
        &self,
        reply_key: ReplyKey,
        reply: DynMessage<A>,
    ) -> anyhow::Result<()> {
        let bytes = postcard::to_stdvec(&Datagram::Reply(reply_key, reply))?;

        self.conn
            .send_datagram(bytes.into())
            .map_err(|e| anyhow::anyhow!("Failed to send reply datagram: {}", e))?;

        Ok(())
    }

    async fn request_out_stream(
        &self,
        actor_id: ActorId,
    ) -> anyhow::Result<iroh::endpoint::SendStream> {
        let bytes = bytes::Bytes::from(postcard::to_stdvec(&Datagram::Actor(actor_id))?);

        self.conn
            .send_datagram(bytes)
            .map_err(|e| anyhow::anyhow!("Failed to send actor request datagram: {}", e))?;

        self.conn
            .open_uni()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to open uni stream for actor {}: {}", actor_id, e))
    }
}

// Deserialization & Import - out_stream

impl<'de, A> Deserialize<'de> for ActorRef<A>
where
    A: RemoteActor,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let (public_key, actor_id): (PublicKey, ActorId) = Deserialize::deserialize(deserializer)?;

        let g_ctx = LocalPeer::get();

        if LocalPeer::get().is_imported(&actor_id) {
            g_ctx
                .get_imported(actor_id)
                .ok_or_else(|| serde::de::Error::custom("Failed to get imported actor"))
        } else {
            let (tx, rx) = mpsc::unbounded_channel();

            g_ctx.arrange_import::<A>(public_key, actor_id, tx.clone(), rx);

            Ok(ActorRef { id: actor_id, tx })
        }
    }
}

// Reply will be encoded into a reply key
// Forward will be encoded into ActorId and handled like regular import with relaying one shot

impl<'de> Deserialize<'de> for Continuation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ContinuationVisitor;

        impl<'de> Visitor<'de> for ContinuationVisitor {
            type Value = Continuation;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("enum Continuation")
            }

            fn visit_enum<A>(self, data: A) -> Result<Self::Value, A::Error>
            where
                A: de::EnumAccess<'de>,
            {
                let (tag, variant) = data.variant::<bool>()?;
                match tag {
                    false => {
                        let mb_reply_key: Option<ReplyKey> = variant.newtype_variant()?;
                        let Some(reply_key) = mb_reply_key else {
                            return Ok(Continuation::nil());
                        };

                        let peer = SENDER.with(|s| s.clone());

                        let (tx, rx) = oneshot::channel();

                        tokio::spawn(async move {
                            let reply = match rx.await {
                                Ok(data) => data,
                                Err(_) => {
                                    #[cfg(feature = "tracing")]
                                    tracing::error!(
                                        "Failed to receive reply for key {}",
                                        reply_key
                                    );
                                    return;
                                }
                            };

                            if let Err(e) = peer.send_reply(reply_key, reply).await {
                                #[cfg(feature = "tracing")]
                                error!("Failed to send reply: {}", e);
                            }
                        });

                        Ok(Continuation::Reply(Some(tx)))
                    }
                    true => {
                        let actor_id: ActorId = variant.newtype_variant()?;

                        let oneshot =
                            LocalPeer::get().arrange_forward_tx(actor_id).map_err(|e| {
                                serde::de::Error::custom(format!(
                                    "Failed to arrange forward: {}",
                                    e
                                ))
                            })?;

                        Ok(Continuation::Forward(actor_id, oneshot))
                    }
                }
            }
        }

        const VARIANTS: &'static [&'static str] = &["Reply", "Forward"];
        deserializer.deserialize_enum("Continuation", VARIANTS, ContinuationVisitor)
    }
}

async fn send_object<T: Serialize>(
    stream: &mut iroh::endpoint::SendStream,
    obj: &T,
) -> anyhow::Result<()> {
    // Serialize, get number of bytes, send it first, then send the object
    let data = postcard::to_stdvec(obj)?;
    let size = data.len() as u64;
    stream.write_all(&size.to_le_bytes()).await?;
    stream.write_all(&data).await?;
    Ok(())
}

async fn recv_object<T: for<'de> Deserialize<'de>>(
    stream: &mut iroh::endpoint::RecvStream,
) -> anyhow::Result<T> {
    let mut size_buf = [0; 8];
    stream.read_exact(&mut size_buf).await?;
    let size = u64::from_le_bytes(size_buf);

    let mut data = vec![0; size as usize];
    stream.read_exact(&mut data).await?;
    let obj = postcard::from_bytes(&data)?;
    Ok(obj)
}

// fn prep_out_streams(new_remotes: HashMap<ActorId, (AnyMsgTx, AnyMsgRx)>) -> anyhow::Result<()> {
//     for (actor_id, tx_rx) in new_remotes {
//         prep_out_stream(actor_id, tx_rx)?;
//     }

//     todo!(
//         "Setup lazy ostreams for new remote actors, so that it can be connected on the first message"
//     )
// }

// fn prep_out_stream(actor_id: ActorId, tx_rx: (AnyMsgTx, AnyMsgRx)) -> anyhow::Result<()> {
//     todo!(
//         "Setup a lazy ostream so that on connection attempt,
//         endpoint send a message for query and try to get uni_stream for sending.

//         Any subsequent message should be forwarded to the stream
//         "
//     )
// }

//  If the remote actor receives a message, it should should check if the uni_stream is connected.
//  If connected, just send the serialized message.
//  If not connected, have endpoint to broadcast it self to looking for the new stream, and wait for the stream.

// type FnForwardOut = fn(stream_tx: &iroh::endpoint::SendStream, any_msg: AnyMsgPack);

// async fn run_out_stream<A>(actor_id: ActorId, msg_rx: MsgRx<A>) {
//     let mut mb_stream_tx: Option<iroh::endpoint::SendStream> = None;

//     while let Some(msg_k) = msg_rx.recv().await {
//         if !mb_stream_tx.is_some() {
//             mb_stream_tx = init_out_stream(actor_id).await;
//         }

//         if let Some(stream_tx) = &mb_stream_tx {
//             let data = serialize_msg_pack(msg_k).await;

//             if let Err(e) = stream_tx.write(data).await {
//                 break;
//             }
//         }
//     }
// }

// async fn init_out_stream(actor_id: ActorId) -> Option<iroh::endpoint::SendStream> {
//     todo!("Request a new stream to the known peers or already known one")
// }

// async fn serialize_msg<A: RemoteActor>(msg: &DynMessage<A>) -> anyhow::Result<&[u8]> {
//     todo!()
// }

// What should do about the continuation?
// Continuation could be either reply or forward

// For the case of reply, it used for ask pattern.
// Which means oneshot should be keeped until the reply comes in
// Where will it come in?
//

// For the case of forward, it could be either local or remote.
// It is not sure.
// However, if it is remote, it would have been imported at some point, and remote actor should be running.
// Sending does not matter,
// But what about the incomming continuation?
// In case of some reply, should mark as reply and managed by reply map.
// In case of forward, it could be either local or remote.
// If it is remote, then it should be once imported by the remote actor.
// So if it could be found in remote actors, it should be local.
// And it could be treated as if regular actor.
// Which means if it is local, it should prepare to get the reply back similar to exporting actor ref.
// So there should be some kind of reply awiting channel should be prepared?
