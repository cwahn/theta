use core::fmt;
use std::{
    any::{Any, type_name},
    collections::HashMap,
    error::Error,
    fmt::{Debug, Display, Formatter},
    hash::Hash,
    marker::PhantomData,
};

use rustc_hash::FxHashMap;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
    ser::SerializeMap,
};
// use serde_flexitos::{DeserializeFn, GetError};

use uuid::Uuid;

use crate::{actor::Actor, message::Message};

pub(crate) type ImplId = Uuid;
pub(crate) type ActorImplId = ImplId;
pub(crate) type BehaviorImplId = ImplId;

// serde modules implementation is minimalization of [https://github.com/Gohla/serde_flexitos]

#[inline]
pub(crate) fn serialize_trait_object<S, I, O>(
    serializer: S,
    id: I,
    trait_object: &O,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    I: Serialize,
    O: erased_serde::Serialize + ?Sized,
{
    SerializeTraitObject { id, trait_object }.serialize(serializer)
}

/// Type alias for deserialize functions of trait object type `O`.
pub(crate) type DeserializeFn<O> =
    for<'de> fn(&mut dyn erased_serde::Deserializer<'de>) -> Result<Box<O>, erased_serde::Error>;

/// Registry mapping unique identifiers of types to their deserialize implementations, enabling deserialization of a
/// specific trait object type.
pub(crate) trait Registry {
    /// The type of unique identifiers this registry uses. `&'static str` is used as a default.
    type Identifier;
    /// The trait object type this registry maps deserialize functions for.
    type TraitObject: ?Sized;

    /// Deserialize a trait object with `deserializer`, using this registry to get the deserialize function for the
    /// concrete type, based on the deserialized ID.
    ///
    /// # Errors
    ///
    /// Returns an error when [get_deserialize_fn](Self::get_deserialize_fn) returns an error for the deserialized ID, or
    /// when deserialization fails.
    #[inline]
    fn deserialize_trait_object<'de, D>(
        &self,
        deserializer: D,
    ) -> Result<Box<Self::TraitObject>, D::Error>
    where
        D: Deserializer<'de>,
        Self: Sized,
        Self::Identifier: Deserialize<'de> + Debug,
    {
        DeserializeTraitObject(self).deserialize(deserializer)
    }

    /// Gets the deserialize function for `id`.
    ///
    /// # Errors
    ///
    /// Implementations may return the following errors:
    ///
    /// - `GetError::NotRegistered { id }` if no deserialize function was registered for `id`.
    /// - `GetError::MultipleRegistrations { id }` if multiple deserialize functions were registered for `id`.
    fn get_deserialize_fn(
        &self,
        id: Self::Identifier,
    ) -> Result<&DeserializeFn<Self::TraitObject>, GetError<Self::Identifier>>;

    /// Gets the trait object name, for diagnostic purposes.
    fn get_trait_object_name(&self) -> &'static str;
}

/// Error while getting deserialize function.
#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
pub(crate) enum GetError<I> {
    /// No deserialize function was registered for `id`.
    NotRegistered { id: I },
    /// Multiple deserialize functions were registered for `id`.
    MultipleRegistrations { id: I },
}
impl<I: Debug> Error for GetError<I> {}
impl<I: Debug> Display for GetError<I> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            GetError::NotRegistered { id } => {
                write!(f, "no deserialize function was registered for id '{id:?}'",)
            }
            GetError::MultipleRegistrations { id } => write!(
                f,
                "multiple deserialize functions were registered for id '{id:?}'",
            ),
        }
    }
}

/// Serialize `trait_object` as a single `id`-`trait_object` pair where `id` is the unique identifier for the concrete
/// type of `trait_object`
pub(crate) struct SerializeTraitObject<'o, I, O: ?Sized> {
    pub(crate) id: I,
    pub(crate) trait_object: &'o O,
}

impl<'a, I, O> Serialize for SerializeTraitObject<'_, I, O>
where
    I: Serialize,
    O: ?Sized + erased_serde::Serialize + 'a,
{
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        /// Wrapper so we can implement [`Serialize`] for `Wrap(O)`.
        #[repr(transparent)]
        struct Wrap<'a, O: ?Sized>(&'a O);
        impl<'a, O> Serialize for Wrap<'a, O>
        where
            O: ?Sized + erased_serde::Serialize + 'a,
        {
            #[inline]
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                erased_serde::serialize(self.0, serializer)
            }
        }

        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry(&self.id, &Wrap(self.trait_object))?;
        map.end()
    }
}

/// Checks whether `T` implements [`erased_serde::Serialize`].
pub(crate) const fn require_erased_serialize_impl<T: ?Sized + erased_serde::Serialize>() {}

/// Deserialize [`Box<<R as Registry>::TraitObject>`](Self::Value) from a single id-value pair, using the registry to
/// get deserialize functions for concrete types of the trait object. Implements [`DeserializeSeed`].
#[repr(transparent)]
pub(crate) struct DeserializeTraitObject<'r, R>(pub(crate) &'r R);

impl<'de, R: Registry> DeserializeSeed<'de> for DeserializeTraitObject<'_, R>
where
    R::Identifier: Deserialize<'de> + Debug,
{
    type Value = Box<R::TraitObject>;

    #[inline]
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de, R: Registry> Visitor<'de> for DeserializeTraitObject<'_, R>
where
    R::Identifier: Deserialize<'de> + Debug,
{
    type Value = Box<R::TraitObject>;

    #[inline]
    fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
        write!(
            formatter,
            "an id-value pair for `Box<dyn {}>`",
            self.0.get_trait_object_name()
        )
    }

    #[inline]
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        // Visit a single id-value pair. Use `IdToDeserializeFn` to deserialize the ID as a string and then visit it,
        // turning it into `deserialize_fn`.
        let Some(deserialize_fn) = map.next_key_seed(IdToDeserializeFn(self.0))? else {
            return Err(de::Error::custom(&self));
        };
        // Use `DeserializeWithFn` to deserialize the value using `deserialize_fn`, resulting in a deserialized value
        // of trait object `O` (or an error).
        map.next_value_seed(DeserializeWithFn(deserialize_fn))
    }
}

impl<R> Copy for DeserializeTraitObject<'_, R> {}
impl<R> Clone for DeserializeTraitObject<'_, R> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'de, R: Registry> Display for DeserializeTraitObject<'_, R>
where
    R::Identifier: Deserialize<'de> + Debug,
{
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.expecting(f)
    }
}

/// Deserialize [`R::Identifier`](Registry::Identifier) and use it to get its deserialize function from the registry.
#[repr(transparent)]
struct IdToDeserializeFn<'r, R>(&'r R);

impl<'de, R: Registry> DeserializeSeed<'de> for IdToDeserializeFn<'_, R>
where
    R::Identifier: Deserialize<'de> + Debug,
{
    type Value = DeserializeFn<R::TraitObject>;

    #[inline]
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        let id = R::Identifier::deserialize(deserializer)?;
        self.0
            .get_deserialize_fn(id)
            .copied()
            .map_err(|e| de::Error::custom(e))
    }
}

/// Deserialize as `Box<O>` using given [deserialize function](DeserializeFn).
#[repr(transparent)]
pub(crate) struct DeserializeWithFn<O: ?Sized>(pub(crate) DeserializeFn<O>);

impl<'de, O: ?Sized> DeserializeSeed<'de> for DeserializeWithFn<O> {
    type Value = Box<O>;

    #[inline]
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        let mut erased = <dyn erased_serde::Deserializer>::erase(deserializer);
        self.0(&mut erased).map_err(de::Error::custom)
    }
}

/// Deserialize [`Vec<Box<<R as Registry>::TraitObject>>`](Self::Value), using the registry to get deserialize functions
/// for concrete types of the trait object. Implements [`DeserializeSeed`].
#[repr(transparent)]
pub(crate) struct DeserializeVecWithTraitObject<'r, R>(pub(crate) &'r R);

impl<'de, R: Registry> DeserializeSeed<'de> for DeserializeVecWithTraitObject<'_, R>
where
    R::Identifier: Deserialize<'de> + Debug,
{
    type Value = Vec<Box<R::TraitObject>>;

    #[inline]
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de, R: Registry> Visitor<'de> for DeserializeVecWithTraitObject<'_, R>
where
    R::Identifier: Deserialize<'de> + Debug,
{
    type Value = Vec<Box<R::TraitObject>>;

    #[inline]
    fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
        formatter.write_str("a sequence of '")?;
        DeserializeTraitObject(self.0).expecting(formatter)?;
        formatter.write_str("'")
    }

    #[inline]
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut vec = if let Some(capacity) = seq.size_hint() {
            Vec::with_capacity(capacity)
        } else {
            Vec::new()
        };
        while let Some(trait_object) = seq.next_element_seed(DeserializeTraitObject(self.0))? {
            vec.push(trait_object);
        }
        Ok(vec)
    }
}

/// Deserialize `HashMap<K, V>`, using `key_deserialize_seed` to deserialize `K`, and `value_deserialize_seed` to
/// deserialize `V`. Implements [`DeserializeSeed`]. Use the following functions to create instances of this struct:
/// - [trait_object_key](Self::trait_object_key): deserialize map keys as trait objects,
/// - [trait_object_value](Self::trait_object_value): deserialize map values as trait objects,
/// - [trait_object_key_and_value](Self::trait_object_key_and_value): deserialize map keys and values as trait objects.
pub(crate) struct DeserializeMapWith<K, V> {
    key_deserialize_seed: K,
    value_deserialize_seed: V,
}

impl<'k, K, V, R> DeserializeMapWith<DeserializeTraitObject<'k, R>, PhantomData<V>>
where
    K: Eq + Hash + ?Sized,
    R: Registry<TraitObject = K>,
{
    /// Deserialize `HashMap<Box<K>, V>`, deserializing `Box<K>` as a trait object where `K` is the trait object type,
    /// using `registry` to get deserialize functions for concrete types of trait object `K`.
    #[inline]
    pub(crate) fn trait_object_key(registry: &'k R) -> Self {
        Self {
            key_deserialize_seed: DeserializeTraitObject(registry),
            value_deserialize_seed: PhantomData::default(),
        }
    }
}

impl<'v, K, V, R> DeserializeMapWith<PhantomData<K>, DeserializeTraitObject<'v, R>>
where
    K: Eq + Hash,
    V: ?Sized,
    R: Registry<TraitObject = V>,
{
    /// Deserialize `HashMap<K, Box<V>>`, deserializing `Box<V>` as a trait object where `V` is the trait object type,
    /// using `registry` to get deserialize functions for concrete types of trait object `V`.
    #[inline]
    pub(crate) fn trait_object_value(registry: &'v R) -> Self {
        Self {
            key_deserialize_seed: PhantomData::default(),
            value_deserialize_seed: DeserializeTraitObject(registry),
        }
    }
}

impl<'k, 'v, K, RK, V, RV>
    DeserializeMapWith<DeserializeTraitObject<'k, RK>, DeserializeTraitObject<'v, RV>>
where
    K: Eq + Hash + ?Sized,
    V: ?Sized,
    RK: Registry<TraitObject = K>,
    RV: Registry<TraitObject = V>,
{
    /// Deserialize `HashMap<Box<K>, Box<V>>`:
    /// - deserialize `Box<K>` as a trait object where `K` is the trait object type, using `key_registry` to get
    ///   deserialize functions for concrete types of trait object `K`.
    /// - deserialize `Box<V>` as a trait object where `V` is the trait object type, using `value_registry` to get
    ///   deserialize functions for concrete types of trait object `V`.
    #[inline]
    pub(crate) fn trait_object_key_and_value(key_registry: &'k RK, value_registry: &'v RV) -> Self {
        Self {
            key_deserialize_seed: DeserializeTraitObject(key_registry),
            value_deserialize_seed: DeserializeTraitObject(value_registry),
        }
    }
}

impl<'de, K, V> DeserializeSeed<'de> for DeserializeMapWith<K, V>
where
    K: DeserializeSeed<'de> + Copy,
    K::Value: Eq + Hash,
    V: DeserializeSeed<'de> + Copy,
{
    type Value = HashMap<K::Value, V::Value>;

    #[inline]
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de, K, V> Visitor<'de> for DeserializeMapWith<K, V>
where
    K: DeserializeSeed<'de> + Copy,
    K::Value: Eq + Hash,
    V: DeserializeSeed<'de> + Copy,
{
    type Value = HashMap<K::Value, V::Value>;

    #[inline]
    fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
        write!(
            formatter,
            "a map with custom key and value `DeserializeSeed` impls"
        )
    }

    #[inline]
    fn visit_map<A: MapAccess<'de>>(self, mut map_access: A) -> Result<Self::Value, A::Error> {
        let mut map = if let Some(capacity) = map_access.size_hint() {
            HashMap::with_capacity(capacity)
        } else {
            HashMap::new()
        };
        while let Some(key) = map_access.next_key_seed(self.key_deserialize_seed)? {
            let value = map_access.next_value_seed(self.value_deserialize_seed)?;
            map.insert(key, value);
        }
        Ok(map)
    }
}

/// Registry to support trait object deserialization
// #[derive(Debug)]
// pub(crate) struct DeserializeFnRegistry<O: ?Sized>(pub(crate) FxHashMap<ImplId, DeserializeFn<O>>);

// impl<O: ?Sized> DeserializeFnRegistry<O> {
//     pub(crate)fn new() -> Self {
//         Self(FxHashMap::default())
//     }
// }

// impl<O: ?Sized> serde_flexitos::Registry for DeserializeFnRegistry<O> {
//     type Identifier = Uuid;
//     type TraitObject = O;

//     fn register(&mut self, id: Self::Identifier, deserialize_fn: DeserializeFn<Self::TraitObject>) {
//         self.0.insert(id, deserialize_fn);
//     }

//     fn get_deserialize_fn(
//         &self,
//         id: Self::Identifier,
//     ) -> Result<&DeserializeFn<Self::TraitObject>, GetError<Self::Identifier>> {
//         self.0.get(&id).ok_or(GetError::NotRegistered { id })
//     }

//     fn get_trait_object_name(&self) -> &'static str {
//         type_name::<Self::TraitObject>()
//     }
// }

#[derive(Debug)]
pub(crate) struct MsgEntry<A: Actor> {
    pub(crate) deserialize_fn: DeserializeFn<dyn Message<A>>,
    pub(crate) serialize_return_fn: fn(&Box<dyn Any + Send + Sync>) -> anyhow::Result<Vec<u8>>,
}

pub(crate) struct MsgRegistry<A: Actor>(pub(crate) FxHashMap<BehaviorImplId, MsgEntry<A>>);

impl<A: Actor> MsgRegistry<A> {
    pub(crate) fn new() -> Self {
        Self(FxHashMap::default())
    }
}

impl<A: Actor> Registry for MsgRegistry<A> {
    type Identifier = Uuid;
    type TraitObject = dyn Message<A>;

    fn get_deserialize_fn(
        &self,
        id: Self::Identifier,
    ) -> Result<&DeserializeFn<Self::TraitObject>, GetError<Self::Identifier>> {
        self.0
            .get(&id)
            .map(|entry| &entry.deserialize_fn)
            .ok_or(GetError::NotRegistered { id })
    }

    fn get_trait_object_name(&self) -> &'static str {
        type_name::<Self::TraitObject>()
    }
}

// #[cfg(test)]
// mod tests {
//     use serde::{Deserialize, Serialize}; // Add this line
//     use serde_flexitos::Registry;
//     use theta_macros::{impl_id, serde_trait};

//     use super::*;

//     #[serde_trait]
//     // #[actor("d1f8c5b2-3e4f-4a0b-9c6d-7e8f9a0b1c2d")]
//     trait TheTrait: erased_serde::Serialize + TheTraitImplId {
//         fn make_number(&self) -> u32;
//     }

//     #[derive(Serialize, Deserialize)]
//     struct SomeType;

//     #[derive(Serialize, Deserialize)]
//     struct AnotherType;

//     // #[behavior("d1f8c5b2-3e4f-4a0b-9c6d-7e8f9a0b1c2d")]
//     #[impl_id("27bf12bd-73a6-4241-98df-ae2a0e37d3dd")]
//     impl TheTrait for SomeType {
//         fn make_number(&self) -> u32 {
//             42
//         }
//     }

//     #[impl_id("d1f8c5b2-3e4f-4a0b-9c6d-7e8f9a0b1c2d")]
//     impl TheTrait for AnotherType {
//         fn make_number(&self) -> u32 {
//             24
//         }
//     }

//     #[test]
//     fn test_deserializer_map() {
//         // Create, make trait object list, and serialize, deserialize and call method
//         let some_value: Box<dyn TheTrait> = Box::new(SomeType);
//         let another_value: Box<dyn TheTrait> = Box::new(AnotherType);

//         let the_list = vec![some_value, another_value];

//         let serialized = the_list
//             .iter()
//             .map(|v| postcard::to_stdvec(v).unwrap())
//             .collect::<Vec<_>>();

//         let deserialized: Vec<Box<dyn TheTrait>> = serialized
//             .iter()
//             .map(|v| postcard::from_bytes(v).unwrap())
//             .collect();

//         assert_eq!(deserialized.len(), 2);

//         assert_eq!(deserialized[0].make_number(), 42);
//         assert_eq!(deserialized[1].make_number(), 24);
//     }
// }
