use serde::{Deserialize, Deserializer, Serialize, Serializer};
use theta_protocol::core::{Ident, Network, Transport};
use tokio::task_local;
use url::Url;

use crate::{
    actor::Actor,
    binding::BINDINGS,
    message::Continuation,
    prelude::ActorRef,
    remote::peer::{Import, LocalPeer, RemotePeer},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ActorRefDto {
    Local(Ident),                            // Serialized local actor reference
    Remote { host_addr: Url, ident: Ident }, // URL of the foreign actor
}

// static NETWORK: LazyLock<Option<Arc<dyn Network>>> = LazyLock::new(|| None);
// static LOCAL_PEER: LazyLock<RwLock<LocalPeerInner>> = LazyLock::new(|| RwLock::new(todo!()));

// static IMPORTED: LazyLock<RwLock<HashMap<Ident, (Arc<dyn Transport>, AnyActorRef)>>> =
//     LazyLock::new(|| RwLock::default());

task_local! {
    pub(crate) static LOCAL_PEER: LocalPeer;
    pub(crate) static CURRENT_PEER: RemotePeer;
}

// Implementations

impl<A: Actor> Serialize for ActorRef<A> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let ident: Ident = self.id().as_bytes().to_vec().into();

        if let Some(Import {
            peer,
            ident,
            actor: _,
        }) = LOCAL_PEER.get().get_imported::<A>(&ident)
        {
            let dto = ActorRefDto::Remote {
                host_addr: peer.host_addr(),
                ident,
            };
            dto.serialize(serializer)
        } else {
            // Local
            if !BINDINGS.read().unwrap().contains_key(&ident[..]) {
                // Unexported local actor, bind and serialize
                BINDINGS
                    .write()
                    .unwrap()
                    .insert(ident.clone().into(), Box::new(self.clone()));
            }

            let dto = ActorRefDto::Local(ident);
            dto.serialize(serializer)
        }
    }
}

impl<'de, A: Actor> Deserialize<'de> for ActorRef<A> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let dto = ActorRefDto::deserialize(deserializer)?;

        match dto {
            ActorRefDto::Local(ident) => {
                if let Some(Import { actor, .. }) = LOCAL_PEER.get().get_imported::<A>(&ident) {
                    Ok(actor)
                } else {
                    // Not yet imported, import from the transport
                    Ok(CURRENT_PEER.get().import(ident))
                }
            }
            ActorRefDto::Remote { host_addr, ident } => {
                match LOCAL_PEER.get().get_imported(&ident) {
                    Some(Import { actor, .. }) => Ok(actor),
                    None => {
                        // Not yet imported, import
                        LOCAL_PEER.get().import::<A>(host_addr, ident).map_err(|e| {
                            serde::de::Error::custom(format!("Failed to import remote actor: {e}"))
                        })
                    }
                }
            }
        }
    }
}

impl Serialize for Continuation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        todo!()
    }
}

impl<'de> Deserialize<'de> for Continuation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        todo!()
    }
}
