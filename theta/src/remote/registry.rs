use std::{
    any::{Any, type_name},
    sync::LazyLock,
};

use rustc_hash::FxHashMap;

use crate::{
    actor::Actor,
    message::Message,
    remote::serde::{ActorImplId, BehaviorImplId, DeserializeFn, Registry},
};

pub(crate) static ACTOR_REGISTRY: LazyLock<ActorRegistry> = LazyLock::new(|| {
    let mut actor_registry = ActorRegistry::default();
    for register_actor_fn in inventory::iter::<RegisterActorFn> {
        (register_actor_fn.0)(&mut actor_registry);
    }

    actor_registry
});

pub(crate) type ActorRegistry = FxHashMap<ActorImplId, ActorEntry>;

#[derive(Debug)]
pub(crate) struct ActorEntry {
    pub(crate) serialize_fn: fn(&Box<dyn Any + Send + Sync>) -> anyhow::Result<Vec<u8>>,
    pub(crate) msg_registry: Box<dyn Any + Send + Sync>,
}

pub(crate) struct RegisterActorFn(pub(crate) fn(&mut ActorRegistry));

inventory::collect!(RegisterActorFn);

pub(crate) struct MsgRegistry<A: Actor>(pub(crate) FxHashMap<BehaviorImplId, MsgEntry<A>>);

#[derive(Debug)]
pub(crate) struct MsgEntry<A: Actor> {
    pub(crate) deserialize_fn: DeserializeFn<dyn Message<A>>,
    pub(crate) serialize_return_fn: fn(&Box<dyn Any + Send + Sync>) -> anyhow::Result<Vec<u8>>,
}

pub(crate) struct RegisterBehaviorFn<A: Actor>(pub(crate) fn(&mut MsgRegistry<A>));
// Each impl_id macro on Actor will collect its own behaviors, since collecting should be done in the crate where it is defined
// inventory::collect!(RegisterBehavior<A>);

// Implementations

impl<A: Actor> Default for MsgRegistry<A> {
    fn default() -> Self {
        Self(FxHashMap::default())
    }
}

impl<A: Actor> Registry for MsgRegistry<A> {
    type Identifier = BehaviorImplId;
    type TraitObject = dyn Message<A>;

    fn get_deserialize_fn(
        &self,
        id: &Self::Identifier,
    ) -> Option<&DeserializeFn<Self::TraitObject>> {
        self.0.get(id).map(|entry| &entry.deserialize_fn)
    }

    fn get_trait_object_name(&self) -> &'static str {
        type_name::<Self::TraitObject>()
    }
}
