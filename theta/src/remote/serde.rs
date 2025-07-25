use std::{
    any::{Any, type_name},
    cell::RefCell,
    sync::{LazyLock, RwLock},
};

use rustc_hash::FxHashMap;
use serde::{Deserialize, Deserializer, Serialize};
use serde_flexitos::{DeserializeFn, GetError};
use tokio::task_local;
use uuid::Uuid;

use crate::{actor::Actor, prelude::ActorRef};

pub(crate) type ImplId = Uuid;

/// Registry to support trait object deserialization
#[derive(Debug)]
pub(crate) struct DeserializeFnRegistry<O: ?Sized>(pub(crate) FxHashMap<ImplId, DeserializeFn<O>>);

impl<O: ?Sized> DeserializeFnRegistry<O> {
    pub fn new() -> Self {
        Self(FxHashMap::default())
    }
}

impl<O: ?Sized> serde_flexitos::Registry for DeserializeFnRegistry<O> {
    type Identifier = Uuid;
    type TraitObject = O;

    fn register(&mut self, id: Self::Identifier, deserialize_fn: DeserializeFn<Self::TraitObject>) {
        self.0.insert(id, deserialize_fn);
    }

    fn get_deserialize_fn(
        &self,
        id: Self::Identifier,
    ) -> Result<&DeserializeFn<Self::TraitObject>, GetError<Self::Identifier>> {
        self.0.get(&id).ok_or(GetError::NotRegistered { id })
    }

    fn get_trait_object_name(&self) -> &'static str {
        type_name::<Self::TraitObject>()
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize}; // Add this line
    use serde_flexitos::Registry;
    use theta_macros::{impl_id, serde_trait};

    use super::*;

    #[serde_trait]
    trait TheTrait: erased_serde::Serialize + TheTraitImplId {
        fn make_number(&self) -> u32;
    }

    #[derive(Serialize, Deserialize)]
    struct SomeType;

    #[derive(Serialize, Deserialize)]
    struct AnotherType;

    #[impl_id("27bf12bd-73a6-4241-98df-ae2a0e37d3dd")]
    impl TheTrait for SomeType {
        fn make_number(&self) -> u32 {
            42
        }
    }

    #[impl_id("d1f8c5b2-3e4f-4a0b-9c6d-7e8f9a0b1c2d")]
    impl TheTrait for AnotherType {
        fn make_number(&self) -> u32 {
            24
        }
    }

    #[test]
    fn test_deserializer_map() {
        // Create, make trait object list, and serialize, deserialize and call method
        let some_value: Box<dyn TheTrait> = Box::new(SomeType);
        let another_value: Box<dyn TheTrait> = Box::new(AnotherType);

        let the_list = vec![some_value, another_value];

        let serialized = the_list
            .iter()
            .map(|v| postcard::to_stdvec(v).unwrap())
            .collect::<Vec<_>>();

        let deserialized: Vec<Box<dyn TheTrait>> = serialized
            .iter()
            .map(|v| postcard::from_bytes(v).unwrap())
            .collect();

        assert_eq!(deserialized.len(), 2);

        assert_eq!(deserialized[0].make_number(), 42);
        assert_eq!(deserialized[1].make_number(), 24);
    }
}
