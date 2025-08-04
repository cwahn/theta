// use std::{
//     any::Any,
//     sync::{LazyLock, RwLock},
// };

// use futures::channel::oneshot;
// use serde::{Deserialize, Serialize};
// use serde_flexitos::Registry;
// use uuid::Uuid;

// use crate::{
//     actor::{Actor, ActorId},
//     message::OneShot,
//     remote::serde::DeserializeFnRegistry,
// };

// type ReplyKey = Uuid;

// #[derive(Serialize, Deserialize)]
// enum EncodedContinuation {
//     Reply(ReplyKey), // ReplyKey is None when no reply is expected
//     Forward(Box<dyn ForwardTarget>),
// }

// pub trait ForwardTarget: erased_serde::Serialize {
//     fn to_oneshot(&self) -> OneShot;

//     fn impl_id(&self) -> Uuid;
// }

// #[derive(Serialize, Deserialize)]
// pub(crate) struct ForwardTargeImpl<A: Actor> {
//     pub(crate) actor_id: ActorId,
//     _phantom: std::marker::PhantomData<A>,
// }

// impl<A: Actor> ForwardTarget for ForwardTargeImpl<A> {
//     fn to_oneshot(&self) -> OneShot {
//         // OneShot::new(self.actor_id.)
//         // Try to get_or init imported actor ref

//         let actor_ref = todo!("get_or_init imported actor ref");

//         // ? But what will the actor will put into this?
//         // Eventually it should turn to BoxedMsg<A>
//         // However, the caller does not know about the actor type
//         // So the oneshot has to know about the return type.
//         // Or actor should support to get Box<dyn Any + Send> which is one of it's messages
//         // And turn it into BoxedMsg<A>

//         let (tx, rx) = oneshot::channel::<Box<dyn Any + Send>>();

//         tokio::spawn(async move {
//             let Ok(res) = rx.await else {
//                 return; // Cancelled
//             };
//         });

//         todo!()
//     }

//     fn impl_id(&self) -> Uuid {
//         A::__IMPL_ID
//     }
// }

// // Each ActorRef should be able to convert it self to a ForwardTarget

// static DESERIALIZE_FORWARD: LazyLock<RwLock<DeserializeFnRegistry<dyn ForwardTarget>>> =
//     LazyLock::new(|| RwLock::new(DeserializeFnRegistry::new()));

// impl Serialize for dyn ForwardTarget {
//     fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
//     where
//         S: serde::Serializer,
//     {
//         const fn __check_erased_serialize_supertrait<T: ?Sized + ForwardTarget>() {
//             serde_flexitos::ser::require_erased_serialize_impl::<T>();
//         }

//         serde_flexitos::serialize_trait_object(serializer, Uuid::new_v4(), self)
//     }
// }

// impl<'de> Deserialize<'de> for Box<dyn ForwardTarget> {
//     fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
//     where
//         D: serde::Deserializer<'de>,
//     {
//         let registry = DESERIALIZE_FORWARD.read().unwrap();

//         registry.deserialize_trait_object(deserializer)
//     }
// }
