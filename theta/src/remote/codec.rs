use std::{marker::PhantomData, sync::Arc};

use postcard::ser_flavors::{AllocVec, Cobs};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use theta_protocol::core::{Network, Transport};
use url::Url;

use crate::{actor::Actor, message::Continuation, prelude::ActorRef};

enum ActorRefDto {
    Local(Vec<u8>), // Serialized local actor reference
    Foreign(Url),   // URL of the foreign actor
}

pub(crate) trait Encode {
    fn encode<E>(&self, encoder: E) -> Result<E::Ok, E::Error>
    where
        E: EncoderT;
}

pub(crate) trait Decode<'de>: Sized {
    fn decode<D>(decoder: D) -> Result<Self, D::Error>
    where
        D: DecoderT<'de>;
}

trait EncoderT: Serializer {
    /// Current transport
    fn context(&self) -> CodecContext;
}

trait DecoderT<'de>: Deserializer<'de> {
    /// Current transport
    fn context(&self) -> CodecContext;
}

struct Encoder<S: Serializer> {
    base: S,
    context: CodecContext,
}

struct Decoder<'de, D: Deserializer<'de>> {
    base: D,
    context: CodecContext,
    _phantom: std::marker::PhantomData<&'de ()>,
}

#[derive(Clone)]
struct CodecContext {
    network: Arc<dyn Network>,
    transport: Arc<dyn Transport>,
}

// Implementations

impl<A: Actor> Encode for ActorRef<A> {
    fn encode<E>(&self, encoder: E) -> Result<E::Ok, E::Error>
    where
        E: EncoderT,
    {
        todo!()
    }
}

impl<'de, A: Actor> Decode<'de> for ActorRef<A> {
    fn decode<D>(decoder: D) -> Result<Self, D::Error>
    where
        D: DecoderT<'de>,
    {
        todo!()
    }
}

impl Encode for Continuation {
    fn encode<E>(&self, encoder: E) -> Result<E::Ok, E::Error>
    where
        E: EncoderT,
    {
        todo!()
    }
}

impl<'de> Decode<'de> for Continuation {
    fn decode<D>(decoder: D) -> Result<Self, D::Error>
    where
        D: DecoderT<'de>,
    {
        todo!()
    }
}

impl<S: Serializer> EncoderT for Encoder<S> {
    fn context(&self) -> CodecContext {
        self.context.clone()
    }
}

impl<'de, D: Deserializer<'de>> DecoderT<'de> for Decoder<'de, D> {
    fn context(&self) -> CodecContext {
        self.context.clone()
    }
}

impl<S: Serializer> Serializer for Encoder<S> {
    type Ok = S::Ok;
    type Error = S::Error;

    type SerializeSeq = S::SerializeSeq;
    type SerializeTuple = S::SerializeTuple;
    type SerializeTupleStruct = S::SerializeTupleStruct;
    type SerializeTupleVariant = S::SerializeTupleVariant;
    type SerializeMap = S::SerializeMap;
    type SerializeStruct = S::SerializeStruct;
    type SerializeStructVariant = S::SerializeStructVariant;

    #[inline]
    fn serialize_bool(self, v: bool) -> Result<Self::Ok, Self::Error> {
        self.base.serialize_bool(v)
    }

    #[inline]
    fn serialize_i8(self, v: i8) -> Result<Self::Ok, Self::Error> {
        self.base.serialize_i8(v)
    }

    #[inline]
    fn serialize_i16(self, v: i16) -> Result<Self::Ok, Self::Error> {
        self.base.serialize_i16(v)
    }

    #[inline]
    fn serialize_i32(self, v: i32) -> Result<Self::Ok, Self::Error> {
        self.base.serialize_i32(v)
    }

    #[inline]
    fn serialize_i64(self, v: i64) -> Result<Self::Ok, Self::Error> {
        self.base.serialize_i64(v)
    }
    #[inline]
    fn serialize_i128(self, v: i128) -> Result<Self::Ok, Self::Error> {
        self.base.serialize_i128(v)
    }

    #[inline]
    fn serialize_u8(self, v: u8) -> Result<Self::Ok, Self::Error> {
        self.base.serialize_u8(v)
    }

    #[inline]
    fn serialize_u16(self, v: u16) -> Result<Self::Ok, Self::Error> {
        self.base.serialize_u16(v)
    }

    #[inline]
    fn serialize_u32(self, v: u32) -> Result<Self::Ok, Self::Error> {
        self.base.serialize_u32(v)
    }

    #[inline]
    fn serialize_u64(self, v: u64) -> Result<Self::Ok, Self::Error> {
        self.base.serialize_u64(v)
    }

    #[inline]
    fn serialize_u128(self, v: u128) -> Result<Self::Ok, Self::Error> {
        self.base.serialize_u128(v)
    }

    #[inline]
    fn serialize_f32(self, v: f32) -> Result<Self::Ok, Self::Error> {
        self.base.serialize_f32(v)
    }

    #[inline]
    fn serialize_f64(self, v: f64) -> Result<Self::Ok, Self::Error> {
        self.base.serialize_f64(v)
    }

    #[inline]
    fn serialize_char(self, v: char) -> Result<Self::Ok, Self::Error> {
        self.base.serialize_char(v)
    }

    #[inline]
    fn serialize_str(self, v: &str) -> Result<Self::Ok, Self::Error> {
        self.base.serialize_str(v)
    }

    #[inline]
    fn serialize_bytes(self, v: &[u8]) -> Result<Self::Ok, Self::Error> {
        self.base.serialize_bytes(v)
    }

    #[inline]
    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        self.base.serialize_none()
    }

    #[inline]
    fn serialize_some<T: ?Sized>(self, value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: Serialize,
    {
        self.base.serialize_some(value)
    }

    #[inline]
    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        self.base.serialize_unit()
    }

    #[inline]
    fn serialize_unit_struct(self, name: &'static str) -> Result<Self::Ok, Self::Error> {
        self.base.serialize_unit_struct(name)
    }

    #[inline]
    fn serialize_unit_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        self.base
            .serialize_unit_variant(name, variant_index, variant)
    }

    #[inline]
    fn serialize_newtype_struct<T: ?Sized>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: Serialize,
    {
        self.base.serialize_newtype_struct(name, value)
    }

    #[inline]
    fn serialize_newtype_variant<T: ?Sized>(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: Serialize,
    {
        self.base
            .serialize_newtype_variant(name, variant_index, variant, value)
    }

    #[inline]
    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        self.base.serialize_seq(len)
    }

    #[inline]
    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.base.serialize_tuple(len)
    }

    #[inline]
    fn serialize_tuple_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.base.serialize_tuple_struct(name, len)
    }

    #[inline]
    fn serialize_tuple_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        self.base
            .serialize_tuple_variant(name, variant_index, variant, len)
    }

    #[inline]
    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        self.base.serialize_map(len)
    }

    #[inline]
    fn serialize_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        self.base.serialize_struct(name, len)
    }

    #[inline]
    fn serialize_struct_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        self.base
            .serialize_struct_variant(name, variant_index, variant, len)
    }

    #[inline]
    fn collect_seq<I>(self, iter: I) -> Result<Self::Ok, Self::Error>
    where
        I: IntoIterator,
        <I as IntoIterator>::Item: Serialize,
    {
        self.base.collect_seq(iter)
    }

    #[inline]
    fn collect_map<K, V, I>(self, iter: I) -> Result<Self::Ok, Self::Error>
    where
        K: Serialize,
        V: Serialize,
        I: IntoIterator<Item = (K, V)>,
    {
        self.base.collect_map(iter)
    }

    #[inline]
    fn collect_str<T>(self, value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + std::fmt::Display,
    {
        self.base.collect_str(value)
    }

    #[inline]
    fn is_human_readable(&self) -> bool {
        self.base.is_human_readable()
    }
}

impl<'de, D: Deserializer<'de>> Deserializer<'de> for Decoder<'de, D> {
    type Error = <D as Deserializer<'de>>::Error;

    #[inline]
    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_any(visitor)
    }

    #[inline]
    fn deserialize_bool<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_bool(visitor)
    }

    #[inline]
    fn deserialize_i8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_i8(visitor)
    }

    #[inline]
    fn deserialize_i16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_i16(visitor)
    }

    #[inline]
    fn deserialize_i32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_i32(visitor)
    }

    #[inline]
    fn deserialize_i64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_i64(visitor)
    }

    #[inline]
    fn deserialize_i128<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_i128(visitor)
    }

    #[inline]
    fn deserialize_u8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_u8(visitor)
    }

    #[inline]
    fn deserialize_u16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_u16(visitor)
    }

    #[inline]
    fn deserialize_u32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_u32(visitor)
    }

    #[inline]
    fn deserialize_u64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_u64(visitor)
    }

    #[inline]
    fn deserialize_u128<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_u128(visitor)
    }

    #[inline]
    fn deserialize_f32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_f32(visitor)
    }

    #[inline]
    fn deserialize_f64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_f64(visitor)
    }

    #[inline]
    fn deserialize_char<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_char(visitor)
    }

    #[inline]
    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_str(visitor)
    }

    #[inline]
    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_string(visitor)
    }

    #[inline]
    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_bytes(visitor)
    }

    #[inline]
    fn deserialize_byte_buf<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_byte_buf(visitor)
    }

    #[inline]
    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_option(visitor)
    }

    #[inline]
    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_unit(visitor)
    }

    #[inline]
    fn deserialize_unit_struct<V>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_unit_struct(name, visitor)
    }

    #[inline]
    fn deserialize_newtype_struct<V>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_newtype_struct(name, visitor)
    }

    #[inline]
    fn deserialize_seq<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_seq(visitor)
    }

    #[inline]
    fn deserialize_tuple<V>(self, len: usize, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_tuple(len, visitor)
    }

    #[inline]
    fn deserialize_tuple_struct<V>(
        self,
        name: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_tuple_struct(name, len, visitor)
    }

    #[inline]
    fn deserialize_map<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_map(visitor)
    }

    #[inline]
    fn deserialize_struct<V>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_struct(name, fields, visitor)
    }

    #[inline]
    fn deserialize_enum<V>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_enum(name, variants, visitor)
    }

    #[inline]
    fn deserialize_identifier<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_identifier(visitor)
    }

    #[inline]
    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.base.deserialize_ignored_any(visitor)
    }

    #[inline]
    fn is_human_readable(&self) -> bool {
        self.base.is_human_readable()
    }
}

// Blanket implementations

/// All serializable types are also encodable.
impl<T: Serialize> Encode for T {
    fn encode<E>(&self, encoder: E) -> Result<E::Ok, E::Error>
    where
        E: EncoderT,
    {
        self.serialize(encoder)
    }
}

/// All deserializable types are also decodable.
impl<'de, T: Deserialize<'de>> Decode<'de> for T {
    fn decode<D>(decoder: D) -> Result<Self, D::Error>
    where
        D: DecoderT<'de>,
    {
        Self::deserialize(decoder)
    }
}

impl<A: Actor> Encode for Option<ActorRef<A>> {
    fn encode<E>(&self, encoder: E) -> Result<E::Ok, E::Error>
    where
        E: EncoderT,
    {
        match self {
            Some(actor_ref) => actor_ref.encode(encoder),
            None => encoder.serialize_none(),
        }
    }
}

impl<'de, A: Actor> Decode<'de> for Option<ActorRef<A>> {
    fn decode<D>(decoder: D) -> Result<Self, D::Error>
    where
        D: DecoderT<'de>,
    {
        if decoder.is_human_readable() {
            // Deserialize as an Option<ActorRef<A>>
            Ok(Option::deserialize(decoder)?)
        } else {
            // Deserialize as a foreign actor reference
            let url = Url::deserialize(decoder)?;
            Ok(Some(ActorRef::foreign(url)))
        }
    }
}
