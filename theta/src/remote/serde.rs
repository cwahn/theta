use futures::channel::oneshot;
use iroh::PublicKey;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{
    actor::{Actor, ActorId},
    base::Ident,
    context::RootContext,
    message::Continuation,
    prelude::ActorRef,
    remote::{
        base::{RemoteError, ReplyKey, Tag},
        peer::{LocalPeer, PEER},
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

pub(crate) type MsgPackDto<A> = (<A as Actor>::Msg, ContinuationDto);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum ForwardInfo {
    Local {
        ident: Ident,
        tag: Tag,
    },
    Remote {
        public_key: Option<PublicKey>,
        ident: Ident,
        tag: Tag,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum ActorRefDto {
    Local(ActorId), // Serialized local actor reference
    Remote {
        actor_id: ActorId,
        public_key: Option<PublicKey>, // None means second party Some means third party to the recipient
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
    fn from(actor: &ActorRef<A>) -> Self {
        let actor_id = actor.id();

        if let Some(import) = LocalPeer::inst().get_import::<A>(actor_id) {
            // If public_key is the same with the current peer, it should be local on recepient's perspective.
            let public_key = import.peer.public_key();

            if public_key == PEER.with(|p| p.public_key()) {
                ActorRefDto::Local(actor_id)
            } else {
                ActorRefDto::Remote {
                    public_key: Some(public_key),
                    actor_id: import.actor.id(),
                }
            }
        } else {
            // Local
            if !RootContext::is_bound_impl::<A>(actor_id.as_bytes()) {
                RootContext::bind_impl(actor_id.as_bytes().to_vec().into(), actor.clone());
            }

            // Local actor is always second party remote actor to the recipient
            ActorRefDto::Remote {
                public_key: None, // None means second party
                actor_id,
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
        ActorRefDto::deserialize(deserializer)?
            .try_into()
            .map_err(|e| {
                serde::de::Error::custom(format!(
                    "Failed to construct ActorRef from ActorRefDto: {e}"
                ))
            })
    }
}

impl<A: Actor> TryFrom<ActorRefDto> for ActorRef<A> {
    type Error = RemoteError;

    fn try_from(dto: ActorRefDto) -> Result<Self, RemoteError> {
        match dto {
            ActorRefDto::Local(ident) => Ok(RootContext::lookup_local_impl::<A>(&ident)?),
            ActorRefDto::Remote {
                public_key,
                actor_id,
            } => {
                match public_key {
                    None => {
                        // Second party remote actor
                        match LocalPeer::inst().get_import::<A>(actor_id) {
                            Some(import) => Ok(import.actor),
                            None => {
                                let actor = PEER.with(|p| p.import::<A>(actor_id));
                                Ok(actor)
                            }
                        }
                    }

                    Some(public_key) => {
                        // Third party remote actor
                        match LocalPeer::inst().get_import::<A>(actor_id) {
                            Some(import) => Ok(import.actor),
                            None => {
                                let actor = LocalPeer::inst().import::<A>(actor_id, public_key)?;
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
                        let reply_key = PEER.with(|p| p.arrange_recv_reply(reply_bytes_tx));

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
                    public_key: Some(public_key),
                    ident,
                    tag,
                } = &mut info
                    && PEER.with(|p| p.public_key()) == *public_key
                {
                    info = ForwardInfo::Local {
                        ident: ident.clone(),
                        tag: *tag,
                    };
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

                tokio::spawn(PEER.scope(PEER.get(), async move {
                    let Ok(reply_bytes) = bytes_rx.await else {
                        return warn!("Failed to receive reply");
                    };

                    // Use get for lifetime condition
                    if let Err(e) = PEER.get().send_reply(reply_key, reply_bytes).await {
                        warn!("Failed to send remote reply: {e}");
                    }
                }));

                Continuation::BytesReply(PEER.get(), bytes_tx) // Serialized return
            }
            ContinuationDto::Forward(forward_info) => match forward_info {
                ForwardInfo::Local { ident, tag } => {
                    let Ok(actor) = RootContext::lookup_any_local_unchecked(&ident) else {
                        warn!("Local actor reference not found in bindings");
                        return Continuation::Nil;
                    };

                    let (tx, rx) = oneshot::channel::<Vec<u8>>();

                    tokio::spawn({
                        async move {
                            let Ok(bytes) = rx.await else {
                                return warn!("Failed to receive tagged bytes");
                            };

                            if let Err(e) = actor.send_tagged_bytes(tag, bytes) {
                                warn!("Failed to send tagged bytes: {e}");
                            }
                        }
                    });

                    Continuation::BytesForward(PEER.get(), tx)
                }
                ForwardInfo::Remote {
                    public_key,
                    ident,
                    tag,
                } => {
                    let peer = match public_key {
                        None => PEER.get(),
                        Some(public_key) => match LocalPeer::inst().get_or_connect(public_key) {
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

                        if let Err(e) = peer.send_forward(ident, tag, bytes).await {
                            warn!("Failed to send forward: {e}");
                        }
                    });

                    Continuation::BytesForward(PEER.get(), tx)
                }
            },
        }
    }
}
