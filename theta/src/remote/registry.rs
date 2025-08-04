use core::fmt;
use std::fmt::{Debug, Display, Formatter};

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, DeserializeSeed, MapAccess, Visitor},
    ser::SerializeMap,
};

// This module is minimalization of [https://github.com/Gohla/serde_flexitos]

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

pub trait Registry {
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

#[allow(dead_code)]
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
