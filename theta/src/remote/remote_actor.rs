// use std::{
//     any::{self, Any},
//     num::NonZeroU128,
//     sync::{LazyLock, RwLock},
// };

// use erased_serde::serialize;
// use futures::{channel::oneshot, stream};
// use rustc_hash::FxHashMap;
// use tokio::sync::mpsc::UnboundedSender;
// use uuid::Uuid;

// use crate::{
//     actor::{Actor, ActorId},
//     actor_ref::{MsgPack, MsgRx, MsgTx},
//     message::{Continuation, DynMessage},
//     prelude::ActorRef,
// };

// // todo Remove Any

// type AnyMsgTx = Box<dyn Any + Send + Sync>; // type-erased MsgTx<A: Actor> 
// type AnyMsgRx = Box<dyn Any + Send + Sync>; // type-erased MsgRx<A: Actor>

// type AnyMsgPack = (Box<dyn Any + Send + Sync>,Continuation); // type-erased MsgPack<A: Actor>

// trait RemoteActor: Actor {
//     const TYPE_ID: Uuid;
// }

// struct RemoteMsgDeserRes<A: RemoteActor> {
//     msg_k: MsgPack<A>,

//     // If no known remote actor, it creates a new one, and returns
//     new_remotes: FxHashMap<ActorId, (AnyMsgTx, AnyMsgRx)>,
// }

// fn handle_inbound<A: RemoteActor>(data: &[u8], actor: &ActorRef<A>) -> Result<(), anyhow::Error> {
//     let (msg, k) = deser::<A>(data)?;

//     actor
//         .send_dyn(msg, k)
//         .map_err(|e| anyhow::anyhow!("Failed to send message: {}", e))?;

//     Ok(())
// }

// fn deser<A: RemoteActor>(data: &[u8]) -> Result<MsgPack<A>, anyhow::Error> {
//     let deser_res = raw_deser::<A>(data)?;

//     prep_out_streams(deser_res.new_remotes)?;

//     Ok(deser_res.msg_k)
// }

// fn raw_deser<A: RemoteActor>(data: &[u8]) -> Result<RemoteMsgDeserRes<A>, anyhow::Error> {
//     todo!()
// }

// fn prep_out_streams(new_remotes: FxHashMap<ActorId, (AnyMsgTx, AnyMsgRx)>) -> anyhow::Result<()> {
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

// //  If the remote actor receives a message, it should should check if the uni_stream is connected.
// //  If connected, just send the serialized message.
// //  If not connected, have endpoint to broadcast it self to looking for the new stream, and wait for the stream.

// type FnForwardOut = fn(stream_tx: &iroh::endpoint::SendStream, any_msg: AnyMsgPack);

// struct OutStream {
//     msg_tx: AnyMsgTx,
//     task_hdl: tokio::task::JoinHandle<()>,
// }

// static OUT_STREAMS: LazyLock<RwLock<FxHashMap<ActorId, OutStream>>> =
//     LazyLock::new(|| RwLock::new(FxHashMap::default()));

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

// // What should do about the continuation?
// // Continuation could be either reply or another ActorRef
// // In case of reply, it is tivial, but not for the other actor ref.
// // In those cases, it should be another ActorRef,

// // struct DynActorRef {
// //     id: ActorId,
// //     tx: UnboundedSender<AnyMsgPack>,
// // }

// // enum K {
// //     Reply(Continuation),
// //     ActorRef(DynActorRef),
// // }

// // type MBK = Option<K>;

// // Continuation must be changed

// // Continuation must be just a another ActorRef
// // If Uuid is nil, it is none.
// // If

// // pub type MsgTx<A> = UnboundedSender<MsgPack<A>>;
// // pub type Continuation = oneshot::Sender<Box<dyn Any + Send>>;

// // struct NewActorRef<A: Actor> {
// //     id: Uuid, // Could be nil
// //     tx: Option<MsgTx<A>>,
// // }

// // enum NewConginuation<A: Actor> {
// //     Reply(Continuation),
// //     ActorRef(NewActorRef<A>),
// // }

// // When serializing continuation, point is that one does not know if it is self, or another actor.

// enum Continuation2 {
//     Reply(Option<oneshot::Sender<Box<dyn Any + Send>>>),
//     ActorRef(ActorId, oneshot::Sender<Box<dyn Any + Send>>),
// }
