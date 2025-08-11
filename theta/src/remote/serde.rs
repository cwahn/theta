use futures::channel::oneshot;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use theta_protocol::core::Ident;
use tokio::task_local;
use url::Url;

use crate::{
    actor::Actor,
    base::ActorImplId,
    binding::BINDINGS,
    message::Continuation,
    prelude::ActorRef,
    remote::{
        base::{Tag, ReplyKey},
        peer::{Import, LocalPeer, RemotePeer},
    },
    warn,
};


pub trait FromTaggedBytes<'de>: Sized {
    fn from<D: Deserializer<'de>>(tag: Tag, bytes: Vec<u8>) -> Result<Self, D::Error>;
}

pub(crate) struct MsgPackDto<A: Actor>(pub(crate) A::Msg, pub(crate) RemoteContinuation);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ForwardInfo {
    pub(crate) actor_impl_id: ActorImplId,
    pub(crate) host_addr: Option<Url>, // None for local actors
    pub(crate) ident: Ident,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ActorRefDto {
    Local(Ident),                            // Serialized local actor reference
    Remote { host_addr: Url, ident: Ident }, // URL of the foreign actor
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum RemoteContinuation {
    Reply(Option<ReplyKey>),
    Forward(ForwardInfo), // Forwarding information
}



// pub(crate) struct PeerContext {
//     pub(crate) local: LocalPeer,
//     pub(crate) current: RemotePeer,
// }

task_local! {
    pub(crate) static LOCAL_PEER: LocalPeer;
    pub(crate) static CURRENT_PEER: RemotePeer;
    // pub(crate) static PEER_CONTEXT: PeerContext;
}

// Implementations

impl<A: Actor> From<&ActorRef<A>> for ActorRefDto {
    fn from(actor_ref: &ActorRef<A>) -> Self {
        let ident: Ident = actor_ref.id().as_bytes().to_vec().into();

        LOCAL_PEER.with(|local_peer| {
            if let Some(import) = local_peer.get_import::<A>(&ident) {
                ActorRefDto::Remote {
                    host_addr: import.peer.host_addr(),
                    ident: import.ident,
                }
            } else {
                // Local
                if !BINDINGS.read().unwrap().contains_key(&ident[..]) {
                    // Unexported local actor, bind and serialize
                    BINDINGS
                        .write()
                        .unwrap()
                        .insert(ident.clone().into(), Box::new(actor_ref.clone()));
                }

                ActorRefDto::Local(ident)
            }
        })
    }
}

impl<A: Actor> Serialize for ActorRef<A> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        ActorRefDto::from(self).serialize(serializer)
    }
}

impl<'de, A: Actor> Deserialize<'de> for ActorRef<A> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(ActorRefDto::deserialize(deserializer)?.into())
    }
}

impl<A: Actor> From<ActorRefDto> for ActorRef<A> {
    fn from(dto: ActorRefDto) -> Self {
        match dto {
            ActorRefDto::Local(ident) => {
                if let Some(actor_ref) = BINDINGS.read().unwrap().get(&ident[..]) {
                    actor_ref.downcast_ref::<ActorRef<A>>().cloned().unwrap()
                } else {
                    panic!("Local actor reference not found in bindings")
                }
            }
            ActorRefDto::Remote { host_addr, ident } => {
                match LOCAL_PEER.get().get_import::<A>(&ident) {
                    Some(Import { actor, .. }) => actor,
                    None => {
                        // Not yet imported, import from the transport
                        LOCAL_PEER
                            .get()
                            .import::<A>(host_addr, ident)
                            .expect("Failed to import remote actor")
                    }
                }
            }
        }
    }
}

// Continuation
// ! Continuation it self is not serializable, since it has to be consumed

impl From<Continuation> for RemoteContinuation {
    fn from(continuation: Continuation) -> Self {
        match continuation {
            Continuation::Nil => RemoteContinuation::Reply(None),
            Continuation::Reply(tx) => {
                let (reply_bytes_tx, reply_bytes_rx) = oneshot::channel();

                match tx.send(Box::new(reply_bytes_rx)) {
                    Err(_) => {
                        warn!("Failed to send reply bytes rx");
                        RemoteContinuation::Reply(None)
                    }
                    Ok(_) => {
                        let reply_key = CURRENT_PEER.with(|p| p.arrange_recv_reply(reply_bytes_tx));

                        RemoteContinuation::Reply(Some(reply_key))
                    }
                }
            }
            Continuation::Forward(tx) => {
                // Request ForwardDto by sending oneshot.
                // If it is local, send dto and make temporary binding
                // If it is remote, send dto without new bytes oneshot, and terminate the task.

                // ! So the point is is it possible to send forward message to remote without type, but only actor_impl_id and concrete message.

                // let (temp_tx, temp_rx) = oneshot::channel();

                // let Ok(()) = tx.send(Box::new(temp_tx)) else {
                //     warn!("Failed to send forward dto");
                //     return ContinuationDto::Reply(None);
                // };

                // let Ok(dto_info) = temp_rx.await else {
                //     warn!("Failed to receive forward dto");
                //     return ContinuationDto::Reply(None);
                // };

                // ! Just do two step forwarding for now.
                // If there is any there is any better logic, please suggest.

                todo!("Forward target could be either local or remote actor");
            }
            _ => panic!("Only Nil, Reply and Forward continuations are serializable"),
        }
    }
}

impl From<RemoteContinuation> for Continuation {
    fn from(dto: RemoteContinuation) -> Self {
        match dto {
            RemoteContinuation::Reply(mb_reply_key) => {
                let Some(reply_key) = mb_reply_key else {
                    return Continuation::Nil;
                };

                let (bytes_tx, bytes_rx) = oneshot::channel();

                tokio::spawn(async move {
                    let Ok(reply_bytes) = bytes_rx.await else {
                        warn!("Failed to receive reply");
                        return;
                    };

                    // Use get for lifetime condition
                    if let Err(e) = CURRENT_PEER.get().send_reply(reply_key, reply_bytes).await {
                        warn!("Failed to send remote reply: {e}");
                    }
                });

                Continuation::RemoteReply(bytes_tx) // Serialized return
            }
            RemoteContinuation::Forward(ForwardInfo {
                actor_impl_id,
                host_addr,
                ident,
            }) => {
                // Now this is the problem,
                // Return -> B::Msg function is necessary before the serialization.
                // Return type is not known even with A
                // Holds information of B as impl_id

                let (bytes_tx)

                todo!();
            }
        }
    }
}
