use std::sync::Arc;

use futures::channel::oneshot;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use theta_protocol::core::Ident;
use url::Url;

use crate::{
    actor::Actor,
    context::{BINDINGS, Binding, LookupError},
    message::Continuation,
    prelude::ActorRef,
    remote::{
        base::{ReplyKey, Tag},
        peer::{CURRENT_PEER, LocalPeer, RemoteActorExt, RemoteError},
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
pub(crate) type MsgPackDto<A: Actor> = (A::Msg, ContinuationDto);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum ForwardInfo {
    Local {
        ident: Ident,
        tag: Tag,
    },
    Remote {
        host_addr: Option<Url>,
        ident: Ident,
        tag: Tag,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ActorRefDto {
    Local(Ident), // Serialized local actor reference
    Remote {
        host_addr: Option<Url>, // None means second party Some means third party to the recipient
        ident: Ident,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum ContinuationDto {
    Nil,
    Reply(ReplyKey),      // None means self reply
    Forward(ForwardInfo), // Forwarding information
}

// Implementations

impl<A: Actor> From<&ActorRef<A>> for ActorRefDto {
    fn from(actor_ref: &ActorRef<A>) -> Self {
        let ident: Ident = actor_ref.id().as_bytes().to_vec().into();

        if let Some(import) = LocalPeer::inst().get_import::<A>(&ident) {
            // If host_addr is the same with the current peer, it should be local on recepient's perspective.
            let host_addr = import.peer.host_addr();

            if host_addr == CURRENT_PEER.with(|p| p.host_addr()) {
                ActorRefDto::Local(ident)
            } else {
                ActorRefDto::Remote {
                    host_addr: Some(host_addr),
                    ident: import.ident,
                }
            }
        } else {
            // Local
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

            // Local actor is always second party remote actor to the recipient
            ActorRefDto::Remote {
                host_addr: None, // None means second party
                ident,
            }
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
                let bindings = BINDINGS.read().unwrap();

                let Some(binding) = bindings.get(&ident[..]) else {
                    return Err(RemoteError::LookupError(LookupError::NotFound));
                };

                let Some(actor) = binding.actor.as_any().downcast_ref::<ActorRef<A>>() else {
                    return Err(RemoteError::LookupError(LookupError::TypeMismatch));
                };

                Ok(actor.clone())
            }
            ActorRefDto::Remote { host_addr, ident } => {
                match host_addr {
                    None => {
                        // Second party remote actor
                        match LocalPeer::inst().get_import::<A>(&ident) {
                            Some(import) => Ok(import.actor),
                            None => {
                                let actor = CURRENT_PEER.with(|p| p.import::<A>(ident.clone()));
                                Ok(actor)
                            }
                        }
                    }

                    Some(host_addr) => {
                        // Third party remote actor
                        match LocalPeer::inst().get_import::<A>(&ident) {
                            Some(import) => Ok(import.actor),
                            None => {
                                let actor =
                                    LocalPeer::inst().import::<A>(host_addr, ident.clone())?;
                                Ok(actor)
                            }
                        }
                    }
                }
            }
        }
    }
}

// Continuation
// ! Continuation it self is not serializable, since it has to be consumed

// impl From<Continuation> for ContinuationDto {
impl Continuation {
    pub(crate) async fn into_dto(self) -> ContinuationDto {
        match self {
            Continuation::Nil => ContinuationDto::Nil,
            Continuation::Reply(tx) => {
                let (reply_bytes_tx, reply_bytes_rx) = oneshot::channel();

                match tx.send(Box::new(reply_bytes_rx)) {
                    Err(_) => {
                        warn!("Failed to send reply bytes rx");
                        ContinuationDto::Nil
                    }
                    Ok(_) => {
                        let reply_key = CURRENT_PEER.with(|p| p.arrange_recv_reply(reply_bytes_tx));

                        ContinuationDto::Reply(reply_key)
                    }
                }
            }
            Continuation::Forward(tx) => {
                let (info_tx, info_rx) = oneshot::channel::<ForwardInfo>();

                if let Err(_) = tx.send(Box::new(info_tx)) {
                    warn!("Failed to request forward info");
                    return ContinuationDto::Nil;
                };

                // ? How should I get the info?
                let Ok(mut info) = info_rx.await else {
                    warn!("Failed to receive forward info");
                    return ContinuationDto::Nil;
                };

                // If the remote host is the same with the recepient, it is local for recepient's perspective
                if let ForwardInfo::Remote {
                    host_addr: Some(host_addr),
                    ident,
                    tag,
                } = &mut info
                {
                    if CURRENT_PEER.with(|p| p.host_addr()) == *host_addr {
                        info = ForwardInfo::Local {
                            ident: ident.clone(),
                            tag: tag.clone(),
                        };
                    }
                }

                ContinuationDto::Forward(info)
            }
            _ => panic!("Only Nil, Reply and Forward continuations are serializable"),
        }
    }
}

impl From<ContinuationDto> for Continuation {
    fn from(dto: ContinuationDto) -> Self {
        match dto {
            ContinuationDto::Nil => Continuation::Nil,
            ContinuationDto::Reply(reply_key) => {
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
            ContinuationDto::Forward(forward_info) => match forward_info {
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
                    let remote_peer = match host_addr {
                        None => CURRENT_PEER.get(),
                        Some(host_addr) => match LocalPeer::inst().get_or_connect(&host_addr) {
                            Ok(peer) => peer,
                            Err(e) => {
                                warn!("Failed to get or connect to remote peer: {e}");
                                return Continuation::Nil;
                            }
                        },
                    };

                    let (tx, rx) = oneshot::channel::<Vec<u8>>();

                    tokio::spawn(async move {
                        let Ok(bytes) = rx.await else {
                            return warn!("Failed to receive continuation bytes");
                        };

                        if let Err(e) = remote_peer.send_forward(ident, tag, bytes).await {
                            warn!("Failed to send forward: {e}");
                        }
                    });

                    Continuation::BytesForward(tx)
                }
            },
        }
    }
}
