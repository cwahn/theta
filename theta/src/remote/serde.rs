use std::{
    any::{Any, type_name},
    sync::LazyLock,
};

use rustc_hash::FxHashMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::{
    actor::Actor,
    message::Message,
    remote::{
        ActorTypeId, MsgTypeId, Remote,
        registry::{
            DeserializeFn, Registry, require_erased_serialize_impl, serialize_trait_object,
        },
    },
};

pub static ACTOR_REGISTRY: LazyLock<ActorRegistry> = LazyLock::new(|| {
    let mut actor_registry = ActorRegistry::default();
    for register_actor_fn in inventory::iter::<RegisterActorFn> {
        (register_actor_fn.0)(&mut actor_registry);
    }

    actor_registry
});

pub type ActorRegistry = FxHashMap<ActorTypeId, ActorEntry>;

#[derive(Debug)]
pub struct ActorEntry {
    pub serialize_fn: fn(&Box<dyn Any + Send + Sync>) -> anyhow::Result<Vec<u8>>,
    pub behavior_registry: Box<dyn Any + Send + Sync>,
}

pub struct RegisterActorFn(pub fn(&mut ActorRegistry));

inventory::collect!(RegisterActorFn);

pub struct BehaviorRegistry<A: Actor>(pub FxHashMap<MsgTypeId, BehaviorEntry<A>>);

#[derive(Debug)]
pub struct BehaviorEntry<A: Actor> {
    // Serialize concrete message to serialized dyn message
    pub serialize_msg: fn(&Box<dyn Any + Send + Sync>) -> anyhow::Result<Vec<u8>>,
    // Deserialize dyn message
    pub deserialize_msg: DeserializeFn<dyn Message<A>>,
    // Serialize return
    pub serialize_return: fn(&Box<dyn Any + Send + Sync>) -> anyhow::Result<Vec<u8>>,
}

pub struct RegisterBehaviorFn<A: Actor>(pub fn(&mut BehaviorRegistry<A>));
// Each impl_id macro on Actor will collect its own behaviors, since collecting should be done in the crate where it is defined
// inventory::collect!(RegisterBehavior<A>);

// Implementations

impl<A: Actor> Default for BehaviorRegistry<A> {
    fn default() -> Self {
        Self(FxHashMap::default())
    }
}

impl<A: Actor> Registry for BehaviorRegistry<A> {
    type Identifier = MsgTypeId;
    type TraitObject = dyn Message<A>;

    fn get_deserialize_fn(
        &self,
        id: &Self::Identifier,
    ) -> Option<&DeserializeFn<Self::TraitObject>> {
        self.0.get(id).map(|entry| &entry.deserialize_msg)
    }

    fn get_trait_object_name(&self) -> &'static str {
        type_name::<Self::TraitObject>()
    }
}

// Implementation for Message<A>

impl<A> Serialize for dyn Message<A>
where
    A: Actor,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        const fn __check_erased_serialize_supertrait<
            A: Actor,
            T: ?Sized + Message<A> + erased_serde::Serialize,
        >() {
            require_erased_serialize_impl::<T>();
        }

        serialize_trait_object(serializer, self.__type_id(), self)
    }
}

impl<'de, A> Deserialize<'de> for Box<dyn Message<A>>
where
    A: Actor,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let registry = ACTOR_REGISTRY
            .get(&<A as Remote>::TYPE_ID)
            .and_then(|actor_entry| {
                actor_entry
                    .behavior_registry
                    .downcast_ref::<BehaviorRegistry<A>>()
            })
            .ok_or_else(|| {
                de::Error::custom(format!(
                    "Failed to get MsgRegistry for {}",
                    type_name::<A>()
                ))
            })?;

        BehaviorRegistry::<A>::deserialize_trait_object(registry, deserializer)
    }
}
