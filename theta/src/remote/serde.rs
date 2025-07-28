use core::fmt;
use std::{
    any::{Any, type_name},
    collections::HashMap,
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

use uuid::Uuid;

use crate::{actor::Actor, message::Message};

// serde modules implementation is minimalization of [https://github.com/Gohla/serde_flexitos]

pub(crate) type ImplId = Uuid;
pub(crate) type ActorImplId = ImplId;
pub(crate) type BehaviorImplId = ImplId;

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

pub(crate) trait Registry {
    type Identifier;
    type TraitObject: ?Sized;

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

    fn get_deserialize_fn(
        &self,
        id: &Self::Identifier,
    ) -> Option<&DeserializeFn<Self::TraitObject>>;

    /// Gets the trait object name, for diagnostic purposes.
    fn get_trait_object_name(&self) -> &'static str;
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
        let Some(deserialize_fn) = map.next_key_seed(IdToDeserializeFn(self.0))? else {
            return Err(de::Error::custom(&self));
        };

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
        self.0.get_deserialize_fn(&id).copied().ok_or_else(|| {
            de::Error::custom(format!(
                "no deserialize function registered for id '{id:?}'"
            ))
        })
    }
}

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

pub(crate) struct DeserializeMapWith<K, V> {
    key_deserialize_seed: K,
    value_deserialize_seed: V,
}

impl<'k, K, V, R> DeserializeMapWith<DeserializeTraitObject<'k, R>, PhantomData<V>>
where
    K: Eq + Hash + ?Sized,
    R: Registry<TraitObject = K>,
{
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
