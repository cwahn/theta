use std::{
    any::Any,
    fmt,
    sync::{Arc, LazyLock, RwLock},
};

use futures::channel::oneshot;
use iroh::PublicKey;
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
    remote,
};

// todo Remove Any

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
    remote_peers: RwLock<HashMap<PublicKey, RemotePeer>>,

    imports: RwLock<HashMap<ActorId, Import>>,
    pending_reply_txs: RwLock<HashMap<ReplyKey, oneshot::Sender<OneShot>>>,
}

#[derive(Debug, Clone)]
struct RemotePeer {
    conn: iroh::endpoint::Connection,
    exports: Arc<RwLock<HashSet<ActorId>>>,
    pending_exports: Arc<RwLock<HashMap<ActorId, oneshot::Sender<iroh::endpoint::RecvStream>>>>,
    pending_reply_recvs: Arc<RwLock<HashMap<ReplyKey, oneshot::Sender<Vec<u8>>>>>,
}

struct Import {
    tx: AnyMsgTx,
    mb_host: Option<PublicKey>,
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
            .alpns(vec![b"theta".to_vec()])
            .discovery_n0()
            .bind()
            .await?;

        tokio::spawn(Self::run_endpoint(endpoint.clone()));

        Ok(Self {
            public_key,
            endpoint,
            remote_peers: RwLock::new(HashMap::default()),

            imports: RwLock::new(HashMap::default()),
            // pending_imports: RwLock::new(HashMap::default()),
            pending_reply_txs: RwLock::new(HashMap::default()),
            // exports: RwLock::new(HashMap::default()),
            // pending_reply_recvs: RwLock::new(HashMap::default()),
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
                    if let Err(e) = remote_ctx.handle_new_conn(conn).await {
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

    async fn handle_new_conn(&self, conn: iroh::endpoint::Connection) -> anyhow::Result<()> {
        let public_key = conn.remote_node_id()?;

        let peer = RemotePeer::init(conn);

        self.remote_peers.write().unwrap().insert(public_key, peer);

        Ok(())
    }

    fn connect_peer(&self, public_key: PublicKey) -> anyhow::Result<()> {
        let mut connections = self.remote_peers.write().unwrap();
        if connections.contains_key(&public_key) {
            return Ok(());
        }

        // let connection = iroh::endpoint::connect(public_key)?;
        let connection = self.endpoint.connect(public_key.clone()).await?;

        tokio::spawn(Self::run_connection(connection.clone()));

        connections.insert(public_key, connection);

        Ok(())
    }

    fn is_imported(&self, actor_id: &ActorId) -> bool {
        self.imports.read().unwrap().contains_key(actor_id)
    }

    fn is_exported(&self, actor_id: &ActorId) -> bool {
        self.exports.read().unwrap().contains_key(actor_id)
    }

    fn get_imported<A: RemoteActor>(&self, actor_id: ActorId) -> Option<ActorRef<A>> {
        let tx = self.get_imported_tx(&actor_id)?;

        Some(ActorRef {
            id: actor_id,
            tx: tx.clone(),
        })
    }

    fn import<A: RemoteActor>(&'static self, actor_id: ActorId, tx: MsgTx<A>, mut rx: MsgRx<A>) {
        // ? Which peer should I connect to?
        tokio::spawn(async move {
            self.register_import(actor_id, tx);

            let Ok(out_stream) = self.request_out_stream(actor_id.clone()).await else {
                tracing::error!("Failed to request out stream for actor {}", actor_id);
                return;
            };

            while let Some(msg_k) = rx.recv().await {
                if let Err(e) = self.handle_outbound(&out_stream, msg_k).await {
                    #[cfg(feature = "tracing")]
                    tracing::error!("Failed to handle outbound message: {}", e);
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
            mb_host: None,
        };

        self.imports.write().unwrap().insert(actor_id, import);
    }

    async fn request_out_stream(
        &self,
        actor_id: ActorId,
    ) -> anyhow::Result<iroh::endpoint::SendStream> {
        let (tx, rx) = oneshot::channel();

        self.pending_imports.write().unwrap().insert(actor_id, tx);

        let (host_pub_key, stream) = rx.await?;

        self.imports
            .write()
            .unwrap()
            .get_mut(&actor_id)
            .map(|import| import.mb_host = Some(host_pub_key));

        Ok(stream)
    }

    async fn handle_outbound<A: RemoteActor>(
        &self,
        out_stream: &iroh::endpoint::SendStream,
        msg_k: MsgPack<A>,
    ) -> anyhow::Result<()> {
        let data = postcard::to_stdvec(&msg_k)?;

        out_stream.write(&data).await?;

        Ok(())
    }

    fn unregister_import(&self, actor_id: &ActorId) {
        self.imports.write().unwrap().remove(actor_id);
    }

    fn arrange_reply_tx<A: RemoteActor>(&self, reply_key: ReplyKey) -> anyhow::Result<OneShot> {
        // todo!("Should return immediately")

        let (tx, rx) = oneshot::channel();

        fn recv_reply<A: RemoteActor>(oneshot: OneShot, data: Vec<u8>) -> anyhow::Result<()> {
            let msg: DynMessage<A> = postcard::from_bytes(&data)?;
            oneshot
                .send(Box::new(msg) as Box<dyn Any + Send>)
                .map_err(|_| anyhow::anyhow!("Failed to send reply"))
        }

        self.awaiting_replies
            .write()
            .unwrap()
            .insert(reply_key, (recv_reply::<A>, tx));

        Ok(rx)
    }

    fn arrange_forward_tx(&self, actor_id: ActorId) -> anyhow::Result<OneShot> {
        todo!("Should return immediately")
    }

    // async fn export<A: RemoteActor>(&self, actor_ref: ActorRef<A>) -> anyhow::Result<()> {
    //     todo!()
    // }
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
            exports: Arc::new(RwLock::new(HashSet::default())),
            pending_exports: Arc::new(RwLock::new(HashMap::default())),
            pending_reply_recvs: Arc::new(RwLock::new(HashMap::default())),
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
        let actor_id = ActorId::deserialize(deserializer)?;

        let g_ctx = LocalPeer::get();

        if LocalPeer::get().is_imported(&actor_id) {
            g_ctx
                .get_imported(actor_id)
                .ok_or_else(|| serde::de::Error::custom("Failed to get imported actor"))
        } else {
            let (tx, rx) = mpsc::unbounded_channel();

            let actor_ref = ActorRef {
                id: actor_id,
                tx: tx.clone(),
            };

            g_ctx.import(actor_id, tx, rx);

            Ok(actor_ref)
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

                        LocalPeer::get()
                            .arrange_reply_tx::<A>(reply_key)
                            .map(|oneshot| Continuation::Reply(Some(oneshot)))
                            .map_err(|e| {
                                serde::de::Error::custom(format!("Failed to arrange reply: {}", e))
                            })
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

fn prep_out_streams(new_remotes: HashMap<ActorId, (AnyMsgTx, AnyMsgRx)>) -> anyhow::Result<()> {
    for (actor_id, tx_rx) in new_remotes {
        prep_out_stream(actor_id, tx_rx)?;
    }

    todo!(
        "Setup lazy ostreams for new remote actors, so that it can be connected on the first message"
    )
}

fn prep_out_stream(actor_id: ActorId, tx_rx: (AnyMsgTx, AnyMsgRx)) -> anyhow::Result<()> {
    todo!(
        "Setup a lazy ostream so that on connection attempt,
        endpoint send a message for query and try to get uni_stream for sending.

        Any subsequent message should be forwarded to the stream
        "
    )
}

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
