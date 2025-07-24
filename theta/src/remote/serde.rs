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

#[derive(Debug)]
pub(crate) struct DeserializerMap<O: ?Sized>(pub(crate) FxHashMap<Uuid, DeserializeFn<O>>);

impl<O: ?Sized> serde_flexitos::Registry for DeserializerMap<O> {
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
    use uuid::uuid;

    use super::*;

    #[derive(Serialize, Deserialize)]
    struct SomeType;
    impl SomeType {
        const ID: Uuid = uuid!("27bf12bd-73a6-4241-98df-ae2a0e37d3dd");
    }

    #[derive(Serialize, Deserialize)]
    struct AnotherType;
    impl AnotherType {
        const ID: Uuid = uuid!("b655ce8f-ccd6-4c5a-8fac-dcccc964eb5e");
    }

    trait TheTrait: erased_serde::Serialize {
        fn tid(&self) -> Uuid;
    }
    impl TheTrait for SomeType {
        fn tid(&self) -> Uuid {
            SomeType::ID
        }
    }
    impl TheTrait for AnotherType {
        fn tid(&self) -> Uuid {
            AnotherType::ID
        }
    }

    static THE_TRAIT_DESERIALIZER: LazyLock<DeserializerMap<dyn TheTrait>> = LazyLock::new(|| {
        let mut registry: DeserializerMap<dyn TheTrait> = DeserializerMap(FxHashMap::default());

        registry.register(SomeType::ID, |d| {
            Ok(Box::new(erased_serde::deserialize::<SomeType>(d)?))
        });

        registry.register(AnotherType::ID, |d| {
            Ok(Box::new(erased_serde::deserialize::<AnotherType>(d)?))
        });

        registry
    });

    #[derive(Serialize, Deserialize)]
    struct Wrapper(Uuid);

    impl Serialize for dyn TheTrait {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            const fn __check_erased_serialize_supertrait<T: ?Sized + TheTrait>() {
                serde_flexitos::ser::require_erased_serialize_impl::<T>();
            }

            serde_flexitos::serialize_trait_object(serializer, self.tid(), self)
        }
    }

    impl<'de> Deserialize<'de> for Box<dyn TheTrait> {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            THE_TRAIT_DESERIALIZER.deserialize_trait_object(deserializer)
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
        assert_eq!(deserialized[0].tid(), SomeType::ID);
        assert_eq!(deserialized[1].tid(), AnotherType::ID);
    }
}
