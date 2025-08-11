use futures::channel::oneshot;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use theta_protocol::core::{Ident, Network, Transport};
use tokio::task_local;
use url::Url;

use crate::{
    actor::Actor,
    base::ActorImplId,
    binding::BINDINGS,
    message::Continuation,
    prelude::ActorRef,
    remote::{
        base::ReplyKey,
        peer::{Import, LocalPeer, RemotePeer},
    },
    warn,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ActorRefDto {
    Local(Ident),                            // Serialized local actor reference
    Remote { host_addr: Url, ident: Ident }, // URL of the foreign actor
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ContinuationDto {
    Reply(Option<ReplyKey>),
    Forward {
        actor_impl_id: ActorImplId,
        host_addr: Url,
        ident: Ident,
    },
}

task_local! {
    pub(crate) static LOCAL_PEER: LocalPeer;
    pub(crate) static CURRENT_PEER: RemotePeer;
}

// Implementations

impl<A: Actor> From<&ActorRef<A>> for ActorRefDto {
    fn from(actor_ref: &ActorRef<A>) -> Self {
        let ident: Ident = actor_ref.id().as_bytes().to_vec().into();

        if let Some(Import { peer, ident, .. }) = LOCAL_PEER.get().get_imported::<A>(&ident) {
            ActorRefDto::Remote {
                host_addr: peer.host_addr(),
                ident,
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
                match LOCAL_PEER.get().get_imported::<A>(&ident) {
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
// impl Serialize for Continuation {
//     fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
//     where
//         S: Serializer,
//     {
//         ContinuationDto::from(self).serialize(serializer)
//     }
// }

impl From<Continuation> for ContinuationDto {
    fn from(continuation: Continuation) -> Self {
        match continuation {
            Continuation::Reply(None) => ContinuationDto::Reply(None),
            Continuation::Reply(Some(tx)) => {
                let (reply_bytes_tx, reply_bytes_rx) = oneshot::channel();

                match tx.send(Box::new(reply_bytes_rx)) {
                    Err(_) => {
                        warn!("Failed to send reply bytes rx");
                        ContinuationDto::Reply(None)
                    }
                    Ok(_) => {
                        let reply_key = CURRENT_PEER.get().arrange_recv_reply(reply_bytes_tx);

                        ContinuationDto::Reply(Some(reply_key))
                    }
                }
            }
            Continuation::Forward(tx) => {
                // Request ForwardDto by sending oneshot.
                // If it is local, send dto and make temporary binding
                // If it is remote, send dto without new bytes oneshot, and terminate the task.

                let (temp_tx, temp_rx) = oneshot::channel();

                let Ok(()) = tx.send(Box::new(temp_tx)) else {
                    warn!("Failed to send forward dto");
                    return ContinuationDto::Reply(None);
                };

                let Ok(dto_info) = temp_rx.await else {
                    warn!("Failed to receive forward dto");
                    return ContinuationDto::Reply(None);
                };

                todo!("Forward target could be either local or remote actor");
            }
        }
    }
}

impl<'de> Deserialize<'de> for Continuation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(ContinuationDto::deserialize(deserializer)?.into())
    }
}

impl From<ContinuationDto> for Continuation {
    fn from(dto: ContinuationDto) -> Self {
        todo!()
    }
}
