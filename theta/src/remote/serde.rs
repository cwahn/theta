use std::sync::Arc;

use futures::channel::oneshot;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use theta_protocol::core::Ident;
use url::Url;

use crate::{
    actor::Actor,
    context::{BINDINGS, Binding},
    message::Continuation,
    prelude::ActorRef,
    remote::{
        base::{ReplyKey, Tag},
        peer::{CURRENT_PEER, Import, LocalPeer, RemoteActorExt, RemoteError},
    },
    warn,
};

/// Types that can be deserialized from tag and bytes.
/// - Used for reconstructing [`Actor::Msg`] from [`Message`] implementators.
pub trait FromTaggedBytes: Sized {
    /// Should implement corresponding deserialization logic for each ['Message'] type.
    /// - It is recommended to use auto-implementation via [`theta_macros::actor`].
    fn from(tag: Tag, bytes: &[u8]) -> Result<Self, postcard::Error>;
}

// #[derive(Debug)]
// pub(crate) struct MsgPackDto<A: Actor>(pub(crate) A::Msg, pub(crate) RemoteDto);
pub(crate) type MsgPackDto<A: Actor> = (A::Msg, RemoteDto);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum ForwardInfo {
    Local {
        ident: Ident,
        tag: Tag,
    },
    Remote {
        host_addr: Url,
        ident: Ident,
        tag: Tag,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ActorRefDto {
    Local(Ident),                            // Serialized local actor reference
    Remote { host_addr: Url, ident: Ident }, // URL of the foreign actor
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum RemoteDto {
    Nil,
    Reply(ReplyKey),      // None means self reply
    Forward(ForwardInfo), // Forwarding information
}

// Implementations

impl<A: Actor> From<&ActorRef<A>> for ActorRefDto {
    fn from(actor_ref: &ActorRef<A>) -> Self {
        let ident: Ident = actor_ref.id().as_bytes().to_vec().into();

        if let Some(import) = LocalPeer::inst().get_import::<A>(&ident) {
            ActorRefDto::Remote {
                host_addr: import.peer.host_addr(),
                ident: import.ident,
            }
        } else {
            // Local
            // ! It might not be enough,
            // in order to avoid un necessary actor impl_id lookup, it should also know the task function to run when looking up comes in.
            // And this is the last place holding the type information.
            // 1. Bind and spawn a task to accept incomming uni_stream.
            // 2. Register task function with ident.
            if !BINDINGS.read().unwrap().contains_key(&ident[..]) {
                // Unexported local actor, bind and serialize
                BINDINGS.write().unwrap().insert(
                    ident.clone().into(),
                    Binding {
                        actor: Arc::new(actor_ref.clone()),
                        export_task_fn: <A as RemoteActorExt>::export_task_fn,
                    },
                );
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
        Ok(ActorRefDto::deserialize(deserializer)?
            .try_into()
            .map_err(|e| {
                serde::de::Error::custom(format!(
                    "Failed to construct ActorRef from ActorRefDto: {e}"
                ))
            })?)
    }
}

impl<A: Actor> TryFrom<ActorRefDto> for ActorRef<A> {
    type Error = RemoteError;

    fn try_from(dto: ActorRefDto) -> Result<Self, RemoteError> {
        match dto {
            ActorRefDto::Local(ident) => {
                let Some(binding) = BINDINGS.read().unwrap().get(&ident[..]).cloned() else {
                    warn!("Local actor reference not found in bindings");
                    return Err(RemoteError::ActorNotFound(ident));
                };

                let Some(actor) = binding
                    .actor
                    .as_any()
                    .downcast_ref::<ActorRef<A>>()
                    .cloned()
                else {
                    warn!("Failed to downcast actor reference");
                    return Err(RemoteError::ActorDowncastFail);
                };

                Ok(actor)
            }
            ActorRefDto::Remote { host_addr, ident } => {
                match LocalPeer::inst().get_import::<A>(&ident) {
                    Some(Import { actor, .. }) => Ok(actor),
                    None => {
                        let actor = match LocalPeer::inst().import::<A>(host_addr, ident) {
                            Ok(actor) => actor,
                            Err(e) => {
                                warn!("Failed to import remote actor: {e}");
                                return Err(e);
                            }
                        };

                        Ok(actor)
                    }
                }
            }
        }
    }
}

// Continuation
// ! Continuation it self is not serializable, since it has to be consumed

impl From<Continuation> for RemoteDto {
    fn from(continuation: Continuation) -> Self {
        match continuation {
            Continuation::Nil => RemoteDto::Nil,
            Continuation::Reply(tx) => {
                let (reply_bytes_tx, reply_bytes_rx) = oneshot::channel();

                match tx.send(Box::new(reply_bytes_rx)) {
                    Err(_) => {
                        warn!("Failed to send reply bytes rx");
                        RemoteDto::Nil
                    }
                    Ok(_) => {
                        let reply_key = CURRENT_PEER.with(|p| p.arrange_recv_reply(reply_bytes_tx));

                        RemoteDto::Reply(reply_key)
                    }
                }
            }
            Continuation::Forward(tx) => {
                let (info_tx, info_rx) = oneshot::channel::<ForwardInfo>();

                if let Err(_) = tx.send(Box::new(info_tx)) {
                    warn!("Failed to request forward info");
                    return RemoteDto::Nil;
                };

                // ? How should I get the info?
                let Ok(info) = info_rx.await else {
                    warn!("Failed to receive forward info");
                    return RemoteDto::Nil;
                };

                // If the host is the same with the recepient, it should be turned into local forward
                let info = CURRENT_PEER.with(|p| {
                    if p.host_addr() == info.host_addr {
                        ForwardInfo::Local {
                            ident: info.ident,
                            tag: info.tag,
                        }
                    } else {
                        info
                    }
                });

                RemoteDto::Forward(info)
            }
            _ => panic!("Only Nil, Reply and Forward continuations are serializable"),
        }
    }
}

impl From<RemoteDto> for Continuation {
    fn from(dto: RemoteDto) -> Self {
        match dto {
            RemoteDto::Nil => Continuation::Nil,
            RemoteDto::Reply(reply_key) => {
                let (bytes_tx, bytes_rx) = oneshot::channel();

                tokio::spawn(async move {
                    let Ok(reply_bytes) = bytes_rx.await else {
                        return warn!("Failed to receive reply");
                    };

                    // Use get for lifetime condition
                    if let Err(e) = CURRENT_PEER.get().send_reply(reply_key, reply_bytes).await {
                        warn!("Failed to send remote reply: {e}");
                    }
                });

                Continuation::BytesReply(bytes_tx) // Serialized return
            }
            RemoteDto::Forward(forward_info) => match forward_info {
                ForwardInfo::Local { ident, tag } => {
                    let Some(binding) = BINDINGS.read().unwrap().get(&ident[..]).cloned() else {
                        warn!("Local actor reference not found in bindings");
                        return Continuation::Nil;
                    };

                    let (tx, rx) = oneshot::channel::<Vec<u8>>();

                    tokio::spawn({
                        let actor = binding.actor.clone();

                        async move {
                            let Ok(bytes) = rx.await else {
                                return warn!("Failed to receive tagged bytes");
                            };

                            if let Err(e) = actor.send_tagged_bytes(tag, bytes) {
                                warn!("Failed to send tagged bytes: {e}");
                            }
                        }
                    });

                    Continuation::BytesForward(tx)
                }
                ForwardInfo::Remote {
                    host_addr,
                    ident,
                    tag,
                } => {
                    let remote_peer = match LocalPeer::inst().get_or_connect(&host_addr) {
                        Ok(peer) => peer,
                        Err(e) => {
                            warn!("Failed to get or connect to remote peer: {e}");
                            return Continuation::Nil;
                        }
                    };

                    let (tx, rx) = oneshot::channel::<Vec<u8>>();

                    tokio::spawn({
                        async move {
                            let Ok(bytes) = rx.await else {
                                return warn!("Failed to receive tagged bytes");
                            };

                            if let Err(e) = remote_peer.send_forward(ident, tag, bytes).await {
                                warn!("Failed to send tagged bytes: {e}");
                            }
                        }
                    });

                    Continuation::BytesForward(tx)
                }
            },
        }
    }
}
