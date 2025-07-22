use std::{
    any::Any,
    cell::RefCell,
    sync::{LazyLock, RwLock},
};

use rustc_hash::{FxBuildHasher, FxHashMap};
use serde::{Deserialize, Deserializer, Serialize};
use serde_flexitos::{DeserializeFn, GetError};
use tokio::{sync::mpsc::UnboundedSender, task_local};
use uuid::Uuid;

use crate::{actor::Actor, message::{DynMessage, Message}, prelude::ActorRef};

// > Locality and security mean that in processing a message: an Actor can send
// > messages only to addresses for which it has information by the following
// > means:
// > 1. that it receives in the message
// > 2. that it already had before it received the message
// > 3. that it creates while processing the message.

type DynTx = Box<dyn Any + Send + Sync>;
type DynRx = Box<dyn Any + Send + Sync>;

// Remote actors imported and alive, e.g. might send messages to it
// static Map for UUID -> SocketTx: UnboundedSender
static REMOTE_ACTORS: LazyLock<RwLock<FxHashMap<Uuid, DynTx>>> =
    LazyLock::new(|| RwLock::new(FxHashMap::default()));

// List of local actors exported and alive, e.g. might receive messages from remote actors
// static Map for UUID -> ACtorRefTx
static LOCAL_ACTORS: LazyLock<RwLock<FxHashMap<Uuid, DynRx>>> =
    LazyLock::new(|| RwLock::new(FxHashMap::default()));

task_local! {
    // UUID -> ActorRefTx
    static LOCAL_ACTOR_BUF: RefCell<Option<FxHashMap<Uuid, DynRx>>>;
    // UUID -> (SocketTx, SocketRx)
    static REMOTE_ACTORE_BUF: RefCell<Option<FxHashMap<Uuid, (DynTx, DynRx)>>>;
}


// Okay the problem is that, the registry needs output type, and that is generic
pub(crate) struct MsgTypeRegistry<O: ?Sized>(pub(crate) FxHashMap<Uuid, Option<DeserializeFn<O>>>);




pub(crate) fn serialize_msg<M: Serialize>(
    msg: &M,
) -> Result<(Vec<u8>, FxHashMap<Uuid, DynRx>), postcard::Error> {
    LOCAL_ACTOR_BUF.with(|x| {
        x.borrow_mut().replace(FxHashMap::default());
    });

    let serialized = postcard::to_stdvec(msg)?;

    let local_mailbox_buf = LOCAL_ACTOR_BUF.with(|x| x.borrow_mut().take()).unwrap();

    Ok((serialized, local_mailbox_buf))
}

// pub(crate) fn deserialize_msg<A>(
//     data: &[u8],
// ) -> Result<(DynMessage<A>, FxHashMap<Uuid, (DynTx, DynRx)>), postcard::Error> 
// where
//     A: Actor,
//     DynMessage<A>: for <'de> Deserialize<'de>,  
// {
//     REMOTE_ACTORE_BUF.with(|x| {
//         x.borrow_mut().replace(FxHashMap::default());
//     });

//     // let deserialized = postcard::from_bytes::<M>(data)?;
//     let deserialized = 

//     let remote_actor_buf = REMOTE_ACTORE_BUF.with(|x| x.borrow_mut().take()).unwrap();

//     Ok((deserialized, remote_actor_buf))
// }

// Implementations

impl<O: ?Sized> serde_flexitos::Registry for MsgTypeRegistry<O> {
    type Identifier = Uuid;
    type TraitObject = O;

    fn register(&mut self, id: Self::Identifier, deserialize_fn: DeserializeFn<Self::TraitObject>) {
        self.0.insert(id, Some(deserialize_fn));
    }

    fn get_deserialize_fn(
        &self,
        id: Self::Identifier,
    ) -> Result<&DeserializeFn<Self::TraitObject>, GetError<Self::Identifier>> {
        self.0
            .get(&id)
            .and_then(|v| v.as_ref())
            .ok_or(GetError::NotRegistered { id })
    }

    fn get_trait_object_name(&self) -> &'static str {
        "Message"
    }
}

// ! Must be executed in tokio context
impl<A> Serialize for ActorRef<A>
where
    A: Actor,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Collect the sender in the task-local storage
        let _ = LOCAL_ACTOR_BUF.try_with(|x| {
            if let Some(ref mut map) = *x.borrow_mut() {
                map.entry(self.id)
                    .or_insert_with(|| Box::new(self.tx.clone()));
            }
        });

        // Serialize only the UUID
        self.id.serialize(serializer)
    }
}

impl<'de, A> Deserialize<'de> for ActorRef<A>
where
    A: Actor,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let id = Uuid::deserialize(deserializer)?;
        // It shoule see if there is existing outbound socket for this UUID
        // If there is one, clone the tx and make a new ActorRef{id, tx}
        // If not create a new mpsc::unbounded_channel and register (tx, rx) to the buffer;
        // Then make a new ActorRef{id, tx}

        // let mb_tx = REMOTE_ACTORS.with(|x| {
        //     x.borrow_mut()
        //         .get_mut(&id)
        //         .and_then(|v| v.downcast_mut::<UnboundedSender<DynMessage<A>>>());
        // });

        todo!()
    }
}
