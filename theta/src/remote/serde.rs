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
