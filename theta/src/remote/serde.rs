use std::collections::HashMap;

use rustc_hash::FxBuildHasher;
use serde_flexitos::{DeserializeFn, GetError};
use uuid::Uuid;

pub(crate) struct MsgTypeRegistry<O: ?Sized>(
    pub(crate) HashMap<Uuid, Option<DeserializeFn<O>>, FxBuildHasher>,
);

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
