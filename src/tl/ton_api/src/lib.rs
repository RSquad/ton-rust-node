/*
 * Copyright (C) 2019-2023 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
#![allow(clippy::unreadable_literal)]

use std::{
    any::{type_name, Any},
    convert::TryFrom,
    fmt::{Debug, Formatter},
    hash::{Hash, Hasher},
    io::{self, Read, Write},
    sync::Arc,
};
use ton_block::{
    fail, BlockIdExt, CryptoSignature, CryptoSignaturePair, Ed25519KeyOption, Error, KeyOption,
    Result, ShardIdent, UInt256,
};

macro_rules! _invalid_id {
    ($id:ident) => {
        Err(crate::InvalidConstructor { expected: Self::possible_constructors(), received: $id }
            .into())
    };
}

pub mod secure;
#[rustfmt::skip]
#[allow(non_camel_case_types)]
pub mod ton;
mod ton_prelude;

include!("../../../common/src/info.rs");

#[macro_export]
macro_rules! hex_dump {
    ($data: expr) => {{
        let mut dump = String::new();
        for i in 0..$data.len() {
            dump.push_str(&format!(
                "{:02x}{}",
                $data[i],
                if (i + 1) % 16 == 0 { '\n' } else { ' ' }
            ))
        }
        dump
    }};
}

/// Trait representing TL constructor number (CRC32 calculated from constructor definition string)
pub trait Constructor {
    fn constructor_const() -> u32;
}

struct ConstructorOnly(u32);

impl BareSerialize for ConstructorOnly {
    fn constructor(&self) -> u32 {
        let Self(ret) = self;
        *ret
    }
    fn serialize_bare(&self, _ser: &mut Serializer) -> Result<()> {
        Ok(())
    }
}

/// Struct for handling mismatched constructor number
#[derive(thiserror::Error)]
#[error("unexpected constructor: {self:?}")]
pub struct InvalidConstructor {
    pub expected: Vec<u32>,
    pub received: u32,
}

impl Debug for InvalidConstructor {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        let expected = self
            .expected
            .iter()
            .map(|id| format!("#{:08x}", id))
            .collect::<Vec<String>>()
            .join(", ");
        write!(f, "got #{:08x}, expected one of [{}]", self.received, expected)
    }
}

/// Struct for deserializing TL-scheme objects from any `io::Read`
pub struct Deserializer<'r> {
    reader: &'r mut dyn Read,
    pos: usize,
}

impl<'r> Deserializer<'r> {
    /// Create `Deserializer` with given `io::Read` trait object
    pub fn new(reader: &'r mut dyn Read) -> Self {
        Deserializer { reader, pos: 0 }
    }

    /// Read `ConstructorNumber` from reader
    pub fn read_constructor(&mut self) -> Result<u32> {
        use byteorder::{LittleEndian, ReadBytesExt};
        Ok(self.read_u32::<LittleEndian>()?)
    }

    /// Read bare-serialized TL-object
    #[inline(always)]
    pub fn read_bare<D: BareDeserialize>(&mut self) -> Result<D> {
        D::deserialize_bare(self)
    }

    /// Read boxed-serialized TL-object
    #[inline(always)]
    pub fn read_boxed<D: BoxedDeserialize>(&mut self) -> Result<D> {
        let constructor = self.read_constructor()?;
        D::deserialize_boxed(constructor, self)
    }

    /// Returns default value for type
    #[inline(always)]
    pub fn just_default<D: Default>(&self) -> Result<D> {
        Ok(Default::default())
    }
}

impl Read for Deserializer<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buf).inspect(|&read| {
            self.pos += read;
        })
    }
}

/// Trait for bare type deserialization
pub trait BareDeserialize: Sized {
    /// Read bare-serialized value using `Deserializer`
    fn deserialize_bare(de: &mut Deserializer) -> Result<Self>;
    /// Read bare-serialized value from `u8` array
    fn bare_deserialized_from_bytes(mut bytes: &[u8]) -> Result<Self> {
        Deserializer::new(&mut bytes).read_bare()
    }
}

/// Trait for boxed type deserialization
pub trait BoxedDeserialize: Sized {
    /// Returns all possible constructors of boxed type
    fn possible_constructors() -> Vec<u32>;
    /// Read boxed-serialized value using `Deserializer`
    fn deserialize_boxed(id: u32, de: &mut Deserializer) -> Result<Self>;
    /// Read boxed-serialized value from `u8` array
    fn boxed_deserialized_from_bytes(mut bytes: &[u8]) -> Result<Self> {
        Deserializer::new(&mut bytes).read_boxed()
    }
}

/// Trait for deserializing any value represented `Object` TL type
pub trait BoxedDeserializeDynamic: BoxedDeserialize {
    /// Read boxed type value with given `Id` using `Deserializer`
    fn boxed_deserialize_to_box(id: u32, de: &mut Deserializer) -> Result<TLObject>;
}

impl<D: BoxedDeserialize + AnyBoxedSerialize> BoxedDeserializeDynamic for D {
    fn boxed_deserialize_to_box(id: u32, de: &mut Deserializer) -> Result<TLObject> {
        Ok(D::deserialize_boxed(id, de)?.into_tl_object())
    }
}

/// Struct representing every boxed type for deserializing `Object` TL type
#[derive(Clone, Copy)]
pub struct DynamicDeserializer {
    id: u32,
    type_name: &'static str,
    ton: fn(u32, &mut Deserializer) -> Result<TLObject>,
}

impl DynamicDeserializer {
    #[inline(always)]
    pub fn from<D: BoxedDeserializeDynamic>(id: u32, type_name: &'static str) -> Self {
        DynamicDeserializer { id, type_name, ton: D::boxed_deserialize_to_box }
    }
}

/// Struct for serializing TL-scheme objects into any `io::Write`
pub struct Serializer<'w> {
    writer: &'w mut dyn Write,
}

impl<'w> Serializer<'w> {
    /// Create `Serializer` with given `io::Write` trait object
    pub fn new(writer: &'w mut dyn Write) -> Self {
        Serializer { writer }
    }

    /// Serialize TL id into writer
    pub fn write_constructor(&mut self, id: u32) -> Result<()> {
        use byteorder::{LittleEndian, WriteBytesExt};
        self.write_u32::<LittleEndian>(id)?;
        Ok(())
    }

    /// Serialize TL-object as bare value
    #[inline(always)]
    pub fn write_bare<S: ?Sized + BareSerialize>(&mut self, obj: &S) -> Result<()> {
        obj.serialize_bare(self)
    }

    /// Serialize TL-object as boxed value
    #[inline(always)]
    pub fn write_boxed<S: ?Sized + BoxedSerialize>(&mut self, obj: &S) -> Result<()> {
        let bare = obj.bare_object();
        self.write_constructor(bare.constructor())?;
        self.write_bare(bare)?;
        Ok(())
    }

    #[inline(always)]
    pub fn write_into_boxed<S: BareSerialize>(&mut self, obj: &S) -> Result<()> {
        let constructor = obj.constructor();
        self.write_constructor(constructor)?;
        self.write_bare(obj)?;
        Ok(())
    }
}

impl Write for Serializer<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

/// Trait for bare type serialization
pub trait BareSerialize {
    /// Get constructor id for object (TL id)
    fn constructor(&self) -> u32;
    /// Write object as bare-serialized value using `Serializer`
    fn serialize_bare(&self, ser: &mut Serializer) -> Result<()>;
    /// Write object as bare-serialized value into `Vec<u8>`
    fn bare_serialized_bytes(&self) -> Result<Vec<u8>> {
        let mut buf: Vec<u8> = vec![];
        Serializer::new(&mut buf).write_bare(self)?;
        Ok(buf)
    }
}

/// Trait for boxed type serialization
pub trait BoxedSerialize {
    /// Get bare object
    fn bare_object(&self) -> &dyn BareSerialize;
    /// Serialize boxed type value into `Vec<u8>`
    fn boxed_serialized_bytes(&self) -> Result<Vec<u8>> {
        let mut buf: Vec<u8> = vec![];
        Serializer::new(&mut buf).write_boxed(self)?;
        Ok(buf)
    }
}

/// Trait for representing bare types as boxed type
pub trait IntoBoxed: BareSerialize {
    type Boxed: BoxedSerialize;
    fn into_boxed(self) -> Self::Boxed;
}

/// Trait for representing any boxed type used in `Object` TL type processing
pub trait AnyBoxedSerialize: Any + BoxedSerialize + Debug + Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn clone_boxed(&self) -> TLObject;
    fn into_boxed_any(self: Box<Self>) -> Box<dyn Any + Send>;
    fn into_tl_object(self) -> TLObject;
}

impl<T: Any + BoxedSerialize + Debug + Send + Sync + Clone> AnyBoxedSerialize for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn clone_boxed(&self) -> TLObject {
        TLObject(Box::new(self.clone()))
    }
    fn into_boxed_any(self: Box<Self>) -> Box<dyn Any + Send> {
        self
    }
    fn into_tl_object(self) -> TLObject {
        TLObject(Box::new(self))
    }
}

/// Trait for functional TL types
pub trait Function: AnyBoxedSerialize {
    type Reply: BoxedDeserialize + AnyBoxedSerialize;
}

/// Represents base TL-object type.
pub struct TLObject(Box<dyn AnyBoxedSerialize>);

impl TLObject {
    pub fn is<I: AnyBoxedSerialize>(&self) -> bool {
        self.0.as_any().is::<I>()
    }
    pub fn downcast<I: AnyBoxedSerialize>(self) -> std::result::Result<I, Self> {
        if self.is::<I>() {
            Ok(*self.0.into_boxed_any().downcast::<I>().unwrap())
        } else {
            Err(self)
        }
    }
}

impl Clone for TLObject {
    fn clone(&self) -> Self {
        let Self(src) = self;
        src.clone_boxed()
    }
}

impl Default for TLObject {
    fn default() -> Self {
        unimplemented!()
    }
}

impl PartialEq for TLObject {
    fn eq(&self, _other: &Self) -> bool {
        unimplemented!()
    }
}

impl Hash for TLObject {
    fn hash<H: Hasher>(&self, _state: &mut H) {
        unimplemented!()
    }
}

impl Debug for TLObject {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        let TLObject(inner) = self;
        write!(f, "(TLObject tl_id:#{:08x} {:?}", inner.bare_object().constructor(), inner)
    }
}

impl BoxedDeserialize for TLObject {
    fn possible_constructors() -> Vec<u32> {
        crate::ton::dynamic::BY_NUMBER.keys().cloned().collect()
    }
    fn deserialize_boxed(id: u32, de: &mut Deserializer) -> Result<Self> {
        match crate::ton::dynamic::BY_NUMBER.get(&id) {
            Some(dynamic) => (dynamic.ton)(id, de),
            None => _invalid_id!(id),
        }
    }
}

impl BoxedSerialize for TLObject {
    fn bare_object(&self) -> &dyn BareSerialize {
        let TLObject(x) = self;
        x.bare_object()
    }
}

// BlockIdExt support

impl BareDeserialize for BlockIdExt {
    fn deserialize_bare(de: &mut Deserializer) -> Result<Self> {
        let shard = ShardIdent::with_tagged_prefix(
            de.read_bare::<crate::ton::int>()?,
            de.read_bare::<crate::ton::long>()? as u64,
        )?;
        let ret = Self::with_params(
            shard,
            de.read_bare::<crate::ton::int>()? as u32,
            de.read_bare::<UInt256>()?,
            de.read_bare::<UInt256>()?,
        );
        Ok(ret)
    }
}

impl BareSerialize for BlockIdExt {
    fn constructor(&self) -> u32 {
        Self::constructor_const()
    }
    fn serialize_bare(&self, se: &mut Serializer) -> Result<()> {
        let shard = self.shard();
        se.write_bare::<crate::ton::int>(&shard.workchain_id())?;
        se.write_bare::<crate::ton::long>(&(shard.shard_prefix_with_tag() as i64))?;
        se.write_bare::<crate::ton::int>(&(self.seq_no() as i32))?;
        se.write_bare::<UInt256>(self.root_hash())?;
        se.write_bare::<UInt256>(self.file_hash())?;
        Ok(())
    }
}

impl BoxedDeserialize for BlockIdExt {
    fn possible_constructors() -> Vec<u32> {
        vec![Self::constructor_const()]
    }
    fn deserialize_boxed(id: u32, de: &mut Deserializer) -> Result<Self> {
        if id == Self::constructor_const() {
            de.read_bare()
        } else {
            _invalid_id!(id)
        }
    }
}

impl BoxedSerialize for BlockIdExt {
    fn bare_object(&self) -> &dyn BareSerialize {
        self
    }
}

impl Constructor for BlockIdExt {
    fn constructor_const() -> u32 {
        crate::ton::ton_node::blockidext::TL_TAG
    }
}

// RldpChunk support

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RldpChunk {
    pub transfer_id: UInt256,
    pub fec_type: crate::ton::fec::Type,
    pub part: crate::ton::int,
    pub total_size: crate::ton::long,
    pub seqno: crate::ton::int,
    pub data: Vec<u8>,
}

impl RldpChunk {
    pub fn serialize_bare(&self, se: &mut Serializer) -> Result<()> {
        self.transfer_id.serialize_bare(se)?;
        se.write_boxed::<crate::ton::fec::Type>(&self.fec_type)?;
        se.write_bare::<crate::ton::int>(&self.part)?;
        se.write_bare::<crate::ton::long>(&self.total_size)?;
        se.write_bare::<crate::ton::int>(&self.seqno)?;
        self.data.serialize_bare(se)?;
        Ok(())
    }

    pub fn deserialize_bare(de: &mut Deserializer) -> Result<Self> {
        let ret = RldpChunk {
            transfer_id: UInt256::deserialize_bare(de)?,
            fec_type: de.read_boxed::<crate::ton::fec::Type>()?,
            part: de.read_bare::<crate::ton::int>()?,
            total_size: de.read_bare::<crate::ton::long>()?,
            seqno: de.read_bare::<crate::ton::int>()?,
            data: Vec::deserialize_bare(de)?,
        };
        Ok(ret)
    }
}

// UInt256 support

impl BareDeserialize for UInt256 {
    fn deserialize_bare(de: &mut Deserializer) -> Result<Self> {
        let mut data = [0u8; 32];
        de.read_exact(&mut data)?;
        Ok(Self::with_array(data))
    }
}

impl BareSerialize for UInt256 {
    fn constructor(&self) -> u32 {
        unreachable!()
    }
    fn serialize_bare(&self, se: &mut Serializer) -> Result<()> {
        se.write_all(self.as_slice())?;
        Ok(())
    }
}

fn downcast_with_error<D: AnyBoxedSerialize>(object: TLObject) -> Result<D> {
    match object.downcast() {
        Ok(result) => Ok(result),
        Err(object) => {
            fail!("Want to get {}, but we have TLObject {:?}", type_name::<D>(), object)
        }
    }
}

/// Deserialize boxed TL object from bytes
pub fn deserialize_boxed(bytes: impl AsRef<[u8]>) -> Result<TLObject> {
    let (ret, _) = deserialize_boxed_with_suffix(bytes)?;
    Ok(ret)
}

/// Deserialize boxed TL object from bytes and return suffix position
pub fn deserialize_boxed_with_suffix(bytes: impl AsRef<[u8]>) -> Result<(TLObject, usize)> {
    let mut reader = bytes.as_ref();
    let mut de = Deserializer::new(&mut reader);
    Ok((de.read_boxed()?, de.pos))
}

/// Deserialize bundle of boxed TL objects from bytes
pub fn deserialize_boxed_bundle(bytes: impl AsRef<[u8]>) -> Result<Vec<TLObject>> {
    let mut bytes = bytes.as_ref();
    let mut de = Deserializer::new(&mut bytes);
    let mut ret = Vec::new();
    loop {
        match de.read_boxed::<TLObject>() {
            Ok(object) => ret.push(object),
            Err(err) => {
                if ret.is_empty() {
                    let dump = hex_dump!(bytes);
                    fail!("TL deserialization error: {err}\nObject: {dump}")
                } else {
                    return Ok(ret);
                }
            }
        }
    }
}

/// Deserialize bundle of boxed TL objects from bytes and return suffix position
pub fn deserialize_boxed_bundle_with_suffix(
    bytes: impl AsRef<[u8]>,
) -> Result<(Vec<TLObject>, usize)> {
    let mut reader = bytes.as_ref();
    let mut de = Deserializer::new(&mut reader);
    let mut ret = Vec::new();
    let mut pos = 0;
    loop {
        match de.read_boxed::<TLObject>() {
            Ok(object) => {
                ret.push(object);
                pos = de.pos;
            }
            Err(err) => {
                if ret.is_empty() {
                    let dump = hex_dump!(bytes.as_ref());
                    fail!("TL bundle deserialization error in pos {pos}: {err}\nObject: {dump}")
                } else {
                    return Ok((ret, pos));
                }
            }
        }
    }
}

/// Deserialize boxed TL object from bytes then downcast to given type
pub fn deserialize_typed<D: AnyBoxedSerialize>(bytes: impl AsRef<[u8]>) -> Result<D> {
    let object = deserialize_boxed(bytes)?;
    downcast_with_error(object)
}

/// Deserialize boxed TL object from bytes then downcast to given type and return suffix position
pub fn deserialize_typed_with_suffix<D: AnyBoxedSerialize>(
    bytes: impl AsRef<[u8]>,
) -> Result<(D, usize)> {
    let mut bytes = bytes.as_ref();
    let mut de = Deserializer::new(&mut bytes);
    let object = de.read_boxed::<TLObject>()?;
    let result = downcast_with_error(object)?;
    Ok((result, de.pos))
}

/// Serialize non-boxed TL object into bytes
pub fn serialize_bare<T: BareSerialize>(object: &T) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    Serializer::new(&mut buf).write_into_boxed(object)?;
    Ok(buf)
}

/// Serialize non-boxed TL object into bytes in-place
pub fn serialize_bare_inplace<T: BareSerialize>(buf: &mut Vec<u8>, object: &T) -> Result<()> {
    buf.truncate(0);
    Serializer::new(buf).write_into_boxed(object)
}

/// Serialize boxed TL object into bytes
pub fn serialize_boxed<T: BoxedSerialize>(object: &T) -> Result<Vec<u8>> {
    let mut ret = Vec::new();
    Serializer::new(&mut ret).write_boxed(object)?;
    Ok(ret)
}

/// Serialize boxed TL object into bytes with appending
pub fn serialize_boxed_append<T: BoxedSerialize>(buf: &mut Vec<u8>, object: &T) -> Result<()> {
    Serializer::new(buf).write_boxed(object)?;
    Ok(())
}

/// Serialize boxed TL object into bytes in-place
pub fn serialize_boxed_inplace<T: BoxedSerialize>(buf: &mut Vec<u8>, object: &T) -> Result<()> {
    buf.truncate(0);
    serialize_boxed_append(buf, object)
}

/// Get TL tag from data bytes
pub fn tag_from_data(data: &[u8]) -> u32 {
    if data.len() < 4 {
        0
    } else {
        u32::from_le_bytes([data[0], data[1], data[2], data[3]])
    }
}

impl TryFrom<&Arc<dyn KeyOption>> for ton::PublicKey {
    type Error = Error;
    fn try_from(value: &Arc<dyn KeyOption>) -> Result<Self> {
        let key = UInt256::with_array(value.pub_key()?.try_into()?);
        let key = ton::pub_::publickey::Ed25519 { key }.into_boxed();
        Ok(key)
    }
}

impl TryFrom<&ton::PublicKey> for Arc<dyn KeyOption> {
    type Error = Error;
    fn try_from(value: &ton::PublicKey) -> Result<Self> {
        match value {
            ton::PublicKey::Pub_Ed25519(key) => {
                Ok(Ed25519KeyOption::from_public_key(key.key.as_slice()))
            }
            value => fail!("Unsupported public key type {:?}", value),
        }
    }
}

pub trait Signing
where
    Self: BareSerialize + Sized,
{
    fn signature_mut(&mut self) -> &mut crate::ton::bytes;
    fn sign(mut self, key: &Arc<dyn KeyOption>) -> Result<Self> {
        let signature = std::mem::take(self.signature_mut());
        debug_assert!(signature.is_empty());

        let mut buf = Vec::new();
        Serializer::new(&mut buf).write_into_boxed(&self)?;
        *self.signature_mut() = key.sign(&buf)?;
        Ok(self)
    }
    fn verify(&mut self, key: &Arc<dyn KeyOption>) -> Result<()> {
        let signature = std::mem::take(self.signature_mut());
        debug_assert!(!signature.is_empty());

        let mut buf = Vec::new();
        Serializer::new(&mut buf).write_into_boxed(self)?;
        *self.signature_mut() = signature; // restore object's signature
        key.verify(&buf, self.signature_mut())
    }
}

impl TryInto<CryptoSignaturePair> for ton::ton_node::blocksignature::BlockSignature {
    type Error = Error;
    fn try_into(self) -> Result<CryptoSignaturePair> {
        Ok(CryptoSignaturePair::with_params(
            self.who,
            CryptoSignature::from_bytes(&self.signature)?,
        ))
    }
}

impl TryFrom<&crate::ton::ton_node::shardid::ShardId> for ShardIdent {
    type Error = Error;
    fn try_from(value: &crate::ton::ton_node::shardid::ShardId) -> Result<Self> {
        ShardIdent::with_tagged_prefix(value.workchain, value.shard as u64)
    }
}
impl TryFrom<crate::ton::ton_node::shardid::ShardId> for ShardIdent {
    type Error = Error;
    fn try_from(value: crate::ton::ton_node::shardid::ShardId) -> Result<Self> {
        (&value).try_into()
    }
}

impl From<&ShardIdent> for crate::ton::ton_node::shardid::ShardId {
    fn from(value: &ShardIdent) -> Self {
        crate::ton::ton_node::shardid::ShardId {
            workchain: value.workchain_id(),
            shard: value.shard_prefix_with_tag() as i64,
        }
    }
}
impl From<ShardIdent> for crate::ton::ton_node::shardid::ShardId {
    fn from(value: ShardIdent) -> Self {
        (&value).into()
    }
}
impl From<&ton::Bool> for bool {
    fn from(value: &ton::Bool) -> Self {
        match value {
            ton::Bool::BoolTrue => true,
            ton::Bool::BoolFalse => false,
        }
    }
}
#[cfg(test)]
#[path = "./tests/tests.rs"]
mod tests;
