use futures::channel::oneshot;
use iroh::PublicKey;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tracing::{error, trace, warn};

#[cfg(all(feature = "ts", target_arch = "wasm32"))]
use crate::ts::TsActor;
#[cfg(all(feature = "ts", target_arch = "wasm32"))]
use crate::ts::TsActorRef;
use crate::{
    actor::{Actor, ActorId},
    base::{BindingError, Hex, Ident},
    compat,
    context::RootContext,
    message::Continuation,
    prelude::ActorRef,
    remote::{
        base::Tag,
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
        actor_id: ActorId,
    },
    Second {
        actor_id: ActorId,
    },
    Third {
        actor_id: ActorId,
        public_key: PublicKey,
    },
}

/// Serializable continuation for remote message handling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum ContinuationDto {
    Nil,
    Reply,
    Forward {
        ident: Ident,
        mb_public_key: Option<PublicKey>,
        tag: Tag,
    },
}

pub(crate) type MsgPackDto<A> = (<A as Actor>::Msg, ContinuationDto);

impl<A: Actor> From<&ActorRef<A>> for ActorRefDto {
    fn from(actor: &ActorRef<A>) -> Self {
        let actor_id = actor.id();

        match LocalPeer::inst().get_import_public_key(&actor_id) {
            None => {
                RootContext::bind_impl(*actor_id.as_bytes(), actor.downgrade());

                Self::Second { actor_id }
            }
            Some(public_key) => {
                if PEER.with(|p| p.public_key() == public_key) {
                    Self::First { actor_id }
                } else {
                    Self::Third {
                        public_key,
                        actor_id,
                    }
                }
            }
        }
    }
}

#[cfg(not(all(feature = "ts", target_arch = "wasm32")))]
impl<A: Actor> Serialize for ActorRef<A> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match PEER.try_get() {
            None => self.id().serialize(serializer),
            Some(_) => ActorRefDto::from(self).serialize(serializer),
        }
    }
}

#[cfg(all(feature = "ts", target_arch = "wasm32"))]
impl<A: Actor + TsActor> Serialize for ActorRef<A> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match PEER.try_get() {
            None => {
                let wasm_ref = <A as crate::ts::TsActor>::WasmRef::from_ref(self.clone());
                let js_val: wasm_bindgen::JsValue = wasm_ref.into();

                serde_wasm_bindgen::preserve::serialize(&js_val, serializer)
            }
            Some(_) => ActorRefDto::from(self).serialize(serializer),
        }
    }
}

#[cfg(not(all(feature = "ts", target_arch = "wasm32")))]
impl<'de, A: Actor> Deserialize<'de> for ActorRef<A> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match PEER.try_get() {
            None => {
                let actor_id = ActorId::deserialize(deserializer)?;

                Self::lookup_local_impl(actor_id.as_bytes()).map_err(|e| {
                    serde::de::Error::custom(format!("failed to lookup local ActorRef: {e}"))
                })
            }
            Some(_) => ActorRefDto::deserialize(deserializer)?
                .try_into()
                .map_err(|e| {
                    serde::de::Error::custom(format!(
                        "failed to construct ActorRef from ActorRefDto: {e}"
                    ))
                }),
        }
    }
}

#[cfg(all(feature = "ts", target_arch = "wasm32"))]
impl<'de, A: Actor + TsActor> Deserialize<'de> for ActorRef<A> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match PEER.try_get() {
            Some(_) => ActorRefDto::deserialize(deserializer)?
                .try_into()
                .map_err(|e| {
                    serde::de::Error::custom(format!(
                        "failed to construct ActorRef from ActorRefDto: {e}"
                    ))
                }),
            None => {
                let js_val: wasm_bindgen::JsValue =
                    serde_wasm_bindgen::preserve::deserialize(deserializer)?;
                let wasm_ref: <A as crate::ts::TsActor>::WasmRef = <<A as crate::ts::TsActor>::WasmRef as crate::ts::TsActorRef<
                    A,
                >>::from_js_value(js_val)
                    .map_err(|_| serde::de::Error::custom("expected actor ref wrapper"))?;

                Ok(wasm_ref.inner_ref())
            }
        }
    }
}

impl<A: Actor> TryFrom<ActorRefDto> for ActorRef<A> {
    type Error = BindingError;

    fn try_from(dto: ActorRefDto) -> Result<Self, Self::Error> {
        match dto {
            ActorRefDto::First { actor_id } => Ok(Self::lookup_local_impl(actor_id.as_bytes())?),
            ActorRefDto::Second { actor_id } => {
                match LocalPeer::inst().get_or_import_actor::<A>(actor_id, || PEER.get()) {
                    None => Err(BindingError::DowncastError),
                    Some(actor) => Ok(actor),
                }
            }
            ActorRefDto::Third {
                public_key,
                actor_id,
            } => match LocalPeer::inst().get_or_import_actor::<A>(actor_id, || {
                LocalPeer::inst().get_or_connect_peer(public_key)
            }) {
                None => Err(BindingError::DowncastError),
                Some(actor) => Ok(actor),
            },
        }
    }
}

impl Continuation {
    pub(crate) async fn into_dto(self) -> Option<ContinuationDto> {
        match self {
            Self::Nil => Some(ContinuationDto::Nil),
            Self::Reply(_) => {
                panic!("Reply continuation must be handled at import site, not via into_dto")
            }
            Self::Forward(tx) => {
                let (info_tx, info_rx) = oneshot::channel::<ForwardInfo>();

                if tx.send(Box::new(info_tx)).is_err() {
                    warn!("failed to request forward info");

                    return Some(ContinuationDto::Nil);
                }

                let Ok(info) = info_rx.await else {
                    warn!("failed to receive forward info");

                    return Some(ContinuationDto::Nil);
                };

                let mb_public_key = match LocalPeer::inst().get_import_public_key(&info.actor_id) {
                    None => Some(LocalPeer::inst().public_key()),
                    Some(public_key) => {
                        if PEER.with(|p| p.public_key() == public_key) {
                            None
                        } else {
                            Some(public_key)
                        }
                    }
                };

                Some(ContinuationDto::Forward {
                    ident: *info.actor_id.as_bytes(),
                    mb_public_key,
                    tag: info.tag,
                })
            }
            _ => panic!("only Nil | Reply | Forward continuations are serializable"),
        }
    }
}

#[allow(clippy::fallible_impl_from)]
impl From<ContinuationDto> for Continuation {
    fn from(dto: ContinuationDto) -> Self {
        match dto {
            ContinuationDto::Nil => Self::Nil,
            ContinuationDto::Reply => {
                panic!("Reply ContinuationDto must be handled in export task, not via From")
            }
            ContinuationDto::Forward {
                ident,
                mb_public_key,
                tag,
            } => match mb_public_key {
                None => {
                    let (tx, rx) = oneshot::channel::<Vec<u8>>();

                    compat::spawn({
                        PEER.scope(PEER.get(), async move {
                            trace!(
                                ident = % Hex(&ident), "Scheduling delegated local forwarding"
                            );
                            let Ok(bytes) = rx.await else {
                                return warn!(
                                    ident = % Hex(&ident),
                                    "failed to receive binary local forwarding"
                                );
                            };

                            let any_actor =
                                match RootContext::lookup_any_local_unchecked_impl(&ident) {
                                    Err(err) => {
                                        return error!(
                                            ident = % Hex(&ident), % err,
                                            "failed to find local forward target",
                                        );
                                    }
                                    Ok(any_actor) => any_actor,
                                };

                            if let Err(err) = any_actor.send_tagged_bytes(tag, bytes) {
                                error!(
                                    ident = % Hex(&ident), % err,
                                    "failed to send binary local forwarding"
                                );
                            }
                        })
                    });

                    Self::LocalBinForward {
                        peer: PEER.get(),
                        tx,
                    }
                }
                Some(public_key) => {
                    let (tx, rx) = oneshot::channel::<Vec<u8>>();

                    compat::spawn(PEER.scope(PEER.get(), async move {
                        trace!(
                            ident = % Hex(&ident), host = % PEER.get(),
                            "scheduling delegated remote forwarding"
                        );
                        let Ok(bytes) = rx.await else {
                            return warn!("failed to receive binary remote forwarding");
                        };

                        let target_peer = LocalPeer::inst().get_or_connect_peer(public_key);

                        if let Err(err) = target_peer.send_forward(ident, tag, bytes).await {
                            warn!(
                                ident = % Hex(&ident), host = % PEER.get(), % err,
                                "failed to send binary remote forwarding"
                            );
                        }
                    }));

                    Self::RemoteBinForward {
                        peer: PEER.get(),
                        tx,
                    }
                }
            },
        }
    }
}
