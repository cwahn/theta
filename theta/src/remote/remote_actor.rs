use std::{
    any::Any,
    cell::RefCell,
    sync::{LazyLock, RwLock},
};

use iroh::{PublicKey, endpoint::Connection};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Deserializer};
use tokio::{sync::mpsc, task_local};
use uuid::Uuid;

use crate::{
    actor::{Actor, ActorId},
    actor_ref::{MsgPack, MsgRx, MsgTx},
    message::{Continuation, DynMessage, OneShot},
    prelude::ActorRef,
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

static REMOTE_CTX: LazyLock<RemoteContext> =
    LazyLock::new(|| RemoteContext::new(get_or_init_public_key()));

struct RemoteContext {
    public_key: PublicKey,
    connections: RwLock<HashMap<PublicKey, Connection>>,
    imports: RwLock<HashMap<ActorId, Import>>,
    exports: RwLock<HashMap<ActorId, Export>>,
    awaiting_replies: RwLock<HashMap<ReplyKey, OneShot>>,
}

struct Import {
    tx: AnyMsgTx,
    host: PublicKey,
}

struct Export {
    peers: HashSet<PublicKey>,
}

fn get_or_init_public_key() -> PublicKey {
    todo!("Load public key from storage or create one and store it")
}

impl RemoteContext {
    fn new(public_key: PublicKey) -> Self {
        Self {
            public_key,
            connections: RwLock::new(HashMap::default()),
            imports: RwLock::new(HashMap::default()),
            exports: RwLock::new(HashMap::default()),
            awaiting_replies: RwLock::new(HashMap::default()),
        }
    }

    fn get() -> &'static Self {
        &REMOTE_CTX
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

    fn get_imported_tx<A: RemoteActor>(&self, actor_id: &ActorId) -> Option<MsgTx<A>> {
        self.imports
            .read()
            .unwrap()
            .get(actor_id)
            .and_then(|import| import.tx.downcast_ref::<MsgTx<A>>().cloned())
    }

    // async fn imort(&self, host: PublicKey, actor_id: ActorId) -> anyhow::Result<()> {
    //     todo!(
    //         "Initial import should result tx of right type
    //         However, the type is erased"
    //     )
    // }

    // async fn export<A: RemoteActor>(&self, actor_ref: ActorRef<A>) -> anyhow::Result<()> {
    //     todo!()
    // }
}

// Deserialization & Import - out_stream

struct MsgDeserRes<A: RemoteActor> {
    msg_k: MsgPack<A>,
    // If no known remote actor, it creates a new one, and returns
    new_remotes: HashMap<ActorId, (AnyMsgTx, AnyMsgRx)>,
}

fn handle_inbound<A: RemoteActor>(data: &[u8], actor: &ActorRef<A>) -> anyhow::Result<()> {
    let (msg, k) = deser::<A>(data)?;

    actor
        .send_dyn(msg, k)
        .map_err(|e| anyhow::anyhow!("Failed to send message: {}", e))?;

    Ok(())
}

fn deser<A: RemoteActor>(data: &[u8]) -> anyhow::Result<MsgPack<A>> {
    let deser_res = raw_deser::<A>(data)?;

    prep_out_streams(deser_res.new_remotes)?;

    Ok(deser_res.msg_k)
}

task_local! {
    static NEW_IMPORTS_BUF: RefCell<Option<HashMap<ActorId, (AnyMsgTx, AnyMsgRx)>>>; // Buffer for new imports
}

fn raw_deser<A: RemoteActor>(data: &[u8]) -> anyhow::Result<MsgDeserRes<A>> {
    NEW_IMPORTS_BUF.with(|buf| {
        buf.borrow_mut().replace(HashMap::default());
    });

    let msg_bytes = postcard::from_bytes::<DynMessage<A>>(data)?;

    let new_imports_buf = NEW_IMPORTS_BUF.with(|buf| buf.borrow_mut().take()).unwrap();

    todo!()
}

impl<'de, A> Deserialize<'de> for ActorRef<A>
where
    A: RemoteActor,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let actor_id = ActorId::deserialize(deserializer)?;

        let g_ctx = RemoteContext::get();

        if RemoteContext::get().is_imported(&actor_id) {
            g_ctx
                .get_imported(actor_id)
                .ok_or_else(|| serde::de::Error::custom("Failed to get imported actor"))
        } else {
            let (tx, rx) = mpsc::unbounded_channel();

            let actor_ref = ActorRef {
                id: actor_id,
                tx: tx.clone(),
            };

            NEW_IMPORTS_BUF.with(|buf| {
                if let Some(ref mut map) = *buf.borrow_mut() {
                    map.insert(actor_id, (Box::new(tx), Box::new(rx)));
                }
            });

            Ok(actor_ref)
        }
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

type FnForwardOut = fn(stream_tx: &iroh::endpoint::SendStream, any_msg: AnyMsgPack);

async fn run_out_stream<A>(actor_id: ActorId, msg_rx: MsgRx<A>) {
    let mut mb_stream_tx: Option<iroh::endpoint::SendStream> = None;

    while let Some(msg_k) = msg_rx.recv().await {
        if !mb_stream_tx.is_some() {
            mb_stream_tx = init_out_stream(actor_id).await;
        }

        if let Some(stream_tx) = &mb_stream_tx {
            let data = serialize_msg_pack(msg_k).await;

            if let Err(e) = stream_tx.write(data).await {
                break;
            }
        }
    }
}

async fn init_out_stream(actor_id: ActorId) -> Option<iroh::endpoint::SendStream> {
    todo!("Request a new stream to the known peers or already known one")
}

async fn serialize_msg<A: RemoteActor>(msg: &DynMessage<A>) -> anyhow::Result<&[u8]> {
    todo!()
}

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
