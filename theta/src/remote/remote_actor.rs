use std::{
    any::Any,
    cell::RefCell,
    sync::{LazyLock, Mutex, RwLock},
};

use iroh::{PublicKey, endpoint::Connection};
use rustc_hash::FxHashMap;
use tokio::task_local;
use uuid::Uuid;

use crate::{
    actor::{Actor, ActorId},
    actor_ref::{MsgPack, MsgRx},
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

// trait RemoteMessage {
//     const TYPE_ID: MsgTypeId;
// }

// ActorRef will be serialized as Uuid

static CONNECTIONS: LazyLock<RwLock<FxHashMap<PublicKey, Connection>>> =
    LazyLock::new(|| RwLock::new(FxHashMap::default()));

// Imports

static IMPORTS: LazyLock<RwLock<FxHashMap<ActorId, Import>>> =
    LazyLock::new(|| RwLock::new(FxHashMap::default()));

struct Import {
    tx: AnyMsgTx,
    host: PublicKey,
    out_stream_hdl: tokio::task::JoinHandle<()>,
}

// Exports

static EXPORTS: LazyLock<RwLock<FxHashMap<ActorId, Export>>> =
    LazyLock::new(|| RwLock::new(FxHashMap::default()));

struct Export {
    in_stream_hdls: FxHashMap<PublicKey, tokio::task::JoinHandle<()>>,
}

static AWITING_REPLIES: LazyLock<RwLock<FxHashMap<ReplyKey, OneShot>>> =
    LazyLock::new(|| RwLock::new(FxHashMap::default()));

// Deserialization & Import - out_stream

struct MsgDeserRes<A: RemoteActor> {
    msg_k: MsgPack<A>,
    // If no known remote actor, it creates a new one, and returns
    new_remotes: FxHashMap<ActorId, (AnyMsgTx, AnyMsgRx)>,
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
    static NEW_IMPORTS_BUF: RefCell<Option<FxHashMap<ActorId, (AnyMsgTx, AnyMsgRx)>>>; // Buffer for new imports
}

fn raw_deser<A: RemoteActor>(data: &[u8]) -> anyhow::Result<MsgDeserRes<A>> {
    NEW_IMPORTS_BUF.with(|buf| {
        buf.borrow_mut().replace(FxHashMap::default());
    });

    let msg_bytes = postcard::from_bytes::<DynMessage<A>>(data)?;

    let new_imports_buf = NEW_IMPORTS_BUF.with(|buf| buf.borrow_mut().take()).unwrap();

    todo!()
}

fn prep_out_streams(new_remotes: FxHashMap<ActorId, (AnyMsgTx, AnyMsgRx)>) -> anyhow::Result<()> {
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
