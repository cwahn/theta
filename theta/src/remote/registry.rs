use std::{any::Any, sync::LazyLock};

use rustc_hash::FxHashMap;

use crate::base::ActorImplId;

pub static ACTOR_REGISTRY: LazyLock<ActorRegistry> = LazyLock::new(|| {
    let mut actor_registry = ActorRegistry::default();
    for register_actor_fn in inventory::iter::<RegisterActorFn> {
        (register_actor_fn.0)(&mut actor_registry);
    }

    actor_registry
});

pub type ActorRegistry = FxHashMap<ActorImplId, ActorEntry>;

#[derive(Debug)]
pub struct ActorEntry {
    pub serialize_fn: fn(&Box<dyn Any + Send>) -> anyhow::Result<Vec<u8>>,
    // pub msg_registry: Box<dyn Any + Send>,
}

pub struct RegisterActorFn(pub fn(&mut ActorRegistry));

inventory::collect!(RegisterActorFn);

// pub struct MsgRegistry<A: Actor>(pub FxHashMap<BehaviorImplId, MsgEntry<A>>);

// #[derive(Debug)]
// pub struct MsgEntry<A: Actor> {
//     pub deserialize_fn: DeserializeFn<dyn Message<A>>,
//     pub serialize_return_fn: fn(&Box<dyn Any + Send>) -> anyhow::Result<Vec<u8>>,
// }

// pub struct RegisterBehaviorFn<A: Actor>(pub fn(&mut MsgRegistry<A>));
// Each impl_id macro on Actor will collect its own behaviors, since collecting should be done in the crate where it is defined
// inventory::collect!(RegisterBehavior<A>);

// Implementations

// impl<A: Actor> Default for MsgRegistry<A> {
//     fn default() -> Self {
//         Self(FxHashMap::default())
//     }
// }

// impl<A: Actor> Registry for MsgRegistry<A> {
//     type Identifier = BehaviorImplId;
//     type TraitObject = dyn Message<A>;

//     fn get_deserialize_fn(
//         &self,
//         id: &Self::Identifier,
//     ) -> Option<&DeserializeFn<Self::TraitObject>> {
//         self.0.get(id).map(|entry| &entry.deserialize_fn)
//     }

//     fn get_trait_object_name(&self) -> &'static str {
//         type_name::<Self::TraitObject>()
//     }
// }

// Implementation for Message<A>
