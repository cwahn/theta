use futures::channel::oneshot;
use iroh::PublicKey;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tracing::{error, trace, warn};

use crate::{
    actor::{Actor, ActorId},
    base::{Hex, Ident},
    context::{LookupError, RootContext},
    message::Continuation,
    peer_fmt,
    prelude::ActorRef,
    remote::{
        base::{Key, Tag},
        peer::{LocalPeer, PEER},
    },
};

/// Trait for types that can be deserialized from tagged byte streams.
///
/// This trait enables reconstruction of actor messages from their serialized
/// form in remote communication. Each message type gets a unique tag for
/// identification during deserialization.
///
/// # Usage with `#[actor]` Macro
///
/// The `#[actor]` macro automatically implements this trait for actor message types.
/// Manual implementation is typically not needed.
pub trait FromTaggedBytes: Sized {
    /// Deserialize a value from its tag and byte representation.
    ///
    /// # Arguments
    ///
    /// * `tag` - The message type identifier
    /// * `bytes` - The serialized message data
    fn from(tag: Tag, bytes: &[u8]) -> Result<Self, postcard::Error>;
}

pub(crate) type MsgPackDto<A> = (<A as Actor>::Msg, ContinuationDto);

/// Forwarding information for message routing in remote communication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ForwardInfo {
    pub(crate) actor_id: ActorId,
    pub(crate) tag: Tag,
}

/// Serializable actor reference for remote communication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum ActorRefDto {
    First {
        actor_id: ActorId, // First party to the recipient peer, recipient local actor
    },
    Second {
        actor_id: ActorId, // Second party remote actor to the recipient peer, sender local actor
    },
    Third {
        actor_id: ActorId, // Third party to both this peer and the recipient peer
        public_key: PublicKey,
    },
}

/// Serializable continuation for remote message handling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum ContinuationDto {
    Nil,
    Reply(Key), // None means self reply
    Forward {
        ident: Ident,
        mb_public_key: Option<PublicKey>, // None means second party Some means third party to the recipient
        tag: Tag,
    },
}

// Implementations

impl<A: Actor> From<&ActorRef<A>> for ActorRefDto {
    fn from(actor: &ActorRef<A>) -> Self {
        let actor_id = actor.id();

        match LocalPeer::inst().get_import_public_key(&actor_id) {
            None => {
                // // ! Currently, once exported never get freed and dropped.
                // // todo Need to find way to unbind when no export exists
                // // ? Is there any way to prevent hash table access on every serialization?
                // RootContext::bind_impl(*actor_id.as_bytes(), actor.clone());

                RootContext::bind_impl(*actor_id.as_bytes(), actor.downgrade());
                // todo Need to find way to clean up the binding when no export exists anymore

                ActorRefDto::Second { actor_id }
            }
            Some(public_key) => match PEER.with(|p| p.public_key() == public_key) {
                false => ActorRefDto::Third {
                    public_key,
                    actor_id,
                },
                true => ActorRefDto::First { actor_id }, // Second party remote actor to the recipient peer
            },
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
                    "failed to construct ActorRef from ActorRefDto: {e}"
                ))
            })
    }
}

impl<A: Actor> TryFrom<ActorRefDto> for ActorRef<A> {
    type Error = LookupError; // Failes only when the actor is imported but of different type

    fn try_from(dto: ActorRefDto) -> Result<Self, Self::Error> {
        match dto {
            ActorRefDto::First { actor_id } => Ok(ActorRef::<A>::lookup_local(actor_id)?), // First party local actor
            ActorRefDto::Second { actor_id } => {
                match LocalPeer::inst().get_or_import_actor::<A>(actor_id, || {
                    // Second party remote actor
                    PEER.get()
                }) {
                    None => Err(LookupError::DowncastError), // The actor is imported but of different type
                    Some(actor) => Ok(actor),
                }
            }
            ActorRefDto::Third {
                public_key,
                actor_id,
            } => {
                match LocalPeer::inst().get_or_import_actor::<A>(actor_id, || {
                    LocalPeer::inst().get_or_connect_peer(public_key)
                }) {
                    None => Err(LookupError::DowncastError), // The actor is imported but of different type
                    Some(actor) => Ok(actor),
                }
            }
        }
    }
}

// Continuation
// ! Continuation it self is not serializable, since it has to be consumed
impl Continuation {
    pub(crate) async fn into_dto(self) -> Option<ContinuationDto> {
        match self {
            Continuation::Nil => Some(ContinuationDto::Nil),
            Continuation::Reply(tx) => {
                let (bin_reply_tx, bin_reply_rx) = oneshot::channel();

                match tx.send(Box::new(bin_reply_rx)) {
                    Err(_) => {
                        warn!("failed to send binary reply tx");
                        Some(ContinuationDto::Nil)
                    }
                    Ok(_) => PEER
                        .with(|p| {
                            if p.is_canceled() {
                                warn!(
                                    host = %p,
                                    "failed to arrange recv remote reply"
                                );
                                None
                            } else {
                                Some(p.arrange_recv_reply(bin_reply_tx))
                            }
                        })
                        .map(ContinuationDto::Reply),
                }
            }
            Continuation::Forward(tx) => {
                let (info_tx, info_rx) = oneshot::channel::<ForwardInfo>();

                if tx.send(Box::new(info_tx)).is_err() {
                    warn!("failed to request forward info");
                    return Some(ContinuationDto::Nil);
                };

                let Ok(info) = info_rx.await else {
                    warn!("failed to receive forward info");
                    return Some(ContinuationDto::Nil);
                };

                let mb_public_key = match LocalPeer::inst().get_import_public_key(&info.actor_id) {
                    None => Some(LocalPeer::inst().public_key()), // This peer, second party to the recipient peer
                    Some(public_key) => match PEER.with(|p| p.public_key() == public_key) {
                        false => Some(public_key), // Third party to both this peer and the recipient peer
                        true => None,              // Recipient itself, local to the recipient peer
                    },
                };

                Some(ContinuationDto::Forward {
                    ident: *info.actor_id.as_bytes(),
                    mb_public_key,
                    tag: info.tag,
                })
            }
            _ => panic!("only Nil, Reply and Forward continuations are serializable"),
        }
    }
}

impl From<ContinuationDto> for Continuation {
    fn from(dto: ContinuationDto) -> Self {
        match dto {
            ContinuationDto::Nil => Continuation::Nil,
            ContinuationDto::Reply(key) => Continuation::BinReply {
                peer: PEER.get(),
                key,
            },
            ContinuationDto::Forward {
                ident,
                mb_public_key,
                tag,
            } => match mb_public_key {
                None => {
                    let (tx, rx) = oneshot::channel::<Vec<u8>>();

                    tokio::spawn({
                        async move {
                            trace!(
                                ident = %Hex(&ident),
                                "Scheduling deligated local forwarding"
                            );

                            let Ok(bytes) = rx.await else {
                                return warn!(
                                    ident = %Hex(&ident),
                                    "failed to receive binary local forwarding"
                                );
                            };

                            let any_actor = match RootContext::lookup_any_local_unchecked(ident) {
                                Err(err) => {
                                    return error!(
                                        ident = %Hex(&ident),
                                        %err,
                                        "failed to find local forward target",
                                    );
                                }
                                Ok(any_actor) => any_actor,
                            };

                            if let Err(err) = any_actor.send_tagged_bytes(tag, bytes) {
                                error!(
                                    ident = %Hex(&ident),
                                    %err,
                                    "failed to send binary local forwarding"
                                );
                            }
                        }
                    });

                    Continuation::LocalBinForward {
                        peer: PEER.get(),
                        tx,
                    }
                }
                Some(public_key) => {
                    let (tx, rx) = oneshot::channel::<Vec<u8>>();

                    tokio::spawn(async move {
                        trace!(
                            ident = %Hex(&ident),
                            host = peer_fmt!(&public_key),
                            "Scheduling deligated remote forwarding"
                        );

                        let Ok(bytes) = rx.await else {
                            return warn!("failed to receive binary remote forwarding");
                        };

                        let target_peer = LocalPeer::inst().get_or_connect_peer(public_key);

                        if let Err(err) = target_peer.send_forward(ident, tag, bytes).await {
                            warn!(
                                ident = %Hex(&ident),
                                host = peer_fmt!(&public_key),
                                %err,
                                "failed to send binary remote forwarding"
                            );
                        }
                    });

                    Continuation::RemoteBinForward {
                        peer: PEER.get(),
                        tx,
                    }
                }
            },
        }
    }
}
