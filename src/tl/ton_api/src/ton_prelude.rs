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
#![allow(non_camel_case_types)]

use crate::{
    ton::Bool, BareDeserialize, BareSerialize, BoxedDeserialize, BoxedSerialize, Deserializer,
    Result, Serializer, TLObject,
};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use extfmt::Hexlify;
use ordered_float::OrderedFloat;
use serde_derive::{Deserialize, Serialize};
use std::{
    any::type_name,
    fmt::{Debug, Display, Formatter},
    hash::Hash,
    io::{Read, Write},
};
use ton_block::{base64_encode, error, fail, Ed25519KeyOption};

const MAX_DEBUG_BYTES_LEN: usize = 4;
const MAX_TL_BYTES_LEN: usize = 16 << 20; // 16 MB
const MAX_TL_VECTOR_LEN: usize = 1 << 20; // 1,048,576

macro_rules! impl_byteslike {
    (@common $ty:ident) => {

        impl Debug for $ty {
            fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
                let $ty(data) = self;
                if data.len() <= MAX_DEBUG_BYTES_LEN {
                    write!(f, "<{}>", Hexlify(data))
                } else {
                    write!(f, "<{}... {} bytes>", Hexlify(&data[..MAX_DEBUG_BYTES_LEN]), data.len())
                }
            }
        }

        impl ::std::ops::Deref for $ty {
            type Target = [u8];
            fn deref(&self) -> &[u8] { &self.0 }
        }

        impl ::std::ops::DerefMut for $ty {
            fn deref_mut(&mut self) -> &mut [u8] { &mut self.0 }
        }

    };

    (@arraylike $ty:ident) => {

        impl_byteslike!(@common $ty);

        impl BareDeserialize for $ty {
            fn deserialize_bare(de: &mut Deserializer) -> Result<Self> {
                let mut ret = Self::default();
                de.read_exact(&mut ret.0)?;
                Ok(ret)
            }
        }

        impl BareSerialize for $ty {
            fn constructor(&self) -> u32 {
                unreachable!()
            }
            fn serialize_bare(&self, ser: &mut Serializer) -> Result<()> {
                ser.write_all(&self.0)?;
                Ok(())
            }
        }

    };
}

/// Represents 128-bit unsigned integer.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Hash)]
pub struct int128(pub [u8; 16]);

/// Represents 256-bit unsigned integer.
pub(crate) type int256 = ton_block::UInt256;

/// Represents 512-bit unsigned integer.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct int512(pub [u8; 64]);

impl Default for int512 {
    fn default() -> Self {
        int512([0; 64])
    }
}

impl_byteslike!(@arraylike int128);
impl_byteslike!(@arraylike int512);

#[derive(Debug, Clone, PartialEq, Hash)]
pub struct LengthPrefixed<T>(pub T);

impl<T> From<T> for LengthPrefixed<T> {
    fn from(x: T) -> Self {
        LengthPrefixed(x)
    }
}

impl<T> BareDeserialize for LengthPrefixed<T>
where
    T: BoxedDeserialize,
{
    fn deserialize_bare(de: &mut Deserializer) -> Result<Self> {
        let len = de.read_i32::<LittleEndian>()?;
        if (len < 0) || ((len as usize) > MAX_TL_BYTES_LEN) {
            fail!("Invalid TL LengthPrefixed length {len}")
        }
        let mut buf = vec![0u8; len as usize];
        de.read_exact(&mut buf)?;
        Ok(LengthPrefixed(T::boxed_deserialized_from_bytes(&buf)?))
    }
}

impl<T> BareSerialize for LengthPrefixed<T>
where
    T: BoxedSerialize,
{
    fn constructor(&self) -> u32 {
        self.0.boxed_serialized_bytes().unwrap_or_default().len() as u32
    }
    fn serialize_bare(&self, ser: &mut Serializer) -> Result<()> {
        let inner = self.0.boxed_serialized_bytes()?;
        ser.write_i32::<LittleEndian>(inner.len() as i32)?;
        ser.write_all(&inner)?;
        Ok(())
    }
}

impl From<bool> for Bool {
    fn from(b: bool) -> Self {
        match b {
            false => Bool::BoolFalse,
            true => Bool::BoolTrue,
        }
    }
}

impl From<Bool> for bool {
    fn from(val: Bool) -> Self {
        match val {
            Bool::BoolTrue => true,
            Bool::BoolFalse => false,
        }
    }
}

impl BoxedDeserialize for bool {
    fn possible_constructors() -> Vec<u32> {
        Bool::possible_constructors()
    }
    fn deserialize_boxed(id: u32, de: &mut Deserializer) -> Result<Self> {
        Ok(Bool::deserialize_boxed(id, de)?.into())
    }
}

impl BoxedSerialize for bool {
    fn bare_object(&self) -> &dyn BareSerialize {
        const FALSE: Bool = Bool::BoolFalse;
        const TRUE: Bool = Bool::BoolTrue;
        match *self {
            false => &FALSE,
            true => &TRUE,
        }
        .bare_object()
    }
}

impl BareDeserialize for String {
    fn deserialize_bare(de: &mut Deserializer) -> Result<Self> {
        let vec = de.read_bare::<Vec<u8>>()?;
        Ok(String::from_utf8(vec)?)
    }
}

impl BareSerialize for String {
    fn constructor(&self) -> u32 {
        unreachable!()
    }
    fn serialize_bare(&self, ser: &mut Serializer) -> Result<()> {
        ser.write_bare::<[u8]>(self.as_bytes())?;
        Ok(())
    }
}

impl<T: BoxedDeserialize> BoxedDeserialize for Box<T> {
    fn possible_constructors() -> Vec<u32> {
        T::possible_constructors()
    }
    fn deserialize_boxed(id: u32, de: &mut Deserializer) -> Result<Self> {
        Ok(Box::new(T::deserialize_boxed(id, de)?))
    }
}

impl<T: BoxedSerialize> BoxedSerialize for Box<T> {
    fn bare_object(&self) -> &dyn BareSerialize {
        (**self).bare_object()
    }
}

/// Vector serialization
pub trait Vectored<T> {
    fn deserialize_with(
        de: &mut Deserializer,
        op: fn(&mut Deserializer) -> Result<T>,
    ) -> Result<Vec<T>>
    where
        Self: Sized,
    {
        let count = de.read_i32::<LittleEndian>()?;
        if (count < 0) || ((count as usize) > MAX_TL_VECTOR_LEN) {
            fail!("Invalid TL vector length {count}")
        }
        let mut ret = Vec::new();
        ret.try_reserve_exact(count as usize)
            .map_err(|e| error!("count {} is too big for {}: {}", count, type_name::<Self>(), e))?;
        for _ in 0..count {
            ret.push(op(de)?)
        }
        Ok(ret)
    }

    fn serialize_with(
        &self,
        ser: &mut Serializer,
        op: fn(&mut Serializer, &T) -> Result<()>,
    ) -> Result<()> {
        let vec = self.vector();
        ser.write_i32::<LittleEndian>(vec.len() as i32)?;
        for item in vec.iter() {
            op(ser, item)?
        }
        Ok(())
    }

    fn vector(&self) -> &Vec<T>;
}

/// Vector of bare objects
pub trait VectoredBare<T: BareDeserialize + BareSerialize> {
    fn deserialize(de: &mut Deserializer) -> Result<Vec<T>>
    where
        Self: Sized;
    fn serialize(&self, ser: &mut Serializer) -> Result<()>;
}

/// Vector of boxed objects
pub trait VectoredBoxed<T: BoxedDeserialize + BoxedSerialize> {
    fn deserialize(de: &mut Deserializer) -> Result<Vec<T>>
    where
        Self: Sized;
    fn serialize(&self, ser: &mut Serializer) -> Result<()>;
}

/// Vector of generic objects
pub type vector<T> = Vec<T>;

impl<T> Vectored<T> for Vec<T> {
    fn vector(&self) -> &Vec<T> {
        self
    }
}

impl<T: BareDeserialize + BareSerialize> VectoredBare<T> for Vec<T> {
    fn deserialize(de: &mut Deserializer) -> Result<Vec<T>> {
        <Vec<T> as Vectored<T>>::deserialize_with(de, |de| de.read_bare())
    }
    fn serialize(&self, ser: &mut Serializer) -> Result<()> {
        (self as &dyn Vectored<T>).serialize_with(ser, |ser, item| ser.write_bare(item))
    }
}

impl<T: BoxedDeserialize + BoxedSerialize> VectoredBoxed<T> for Vec<T> {
    fn deserialize(de: &mut Deserializer) -> Result<Vec<T>> {
        <Vec<T> as Vectored<T>>::deserialize_with(de, |de| de.read_boxed())
    }
    fn serialize(&self, ser: &mut Serializer) -> Result<()> {
        (self as &dyn Vectored<T>).serialize_with(ser, |ser, item| ser.write_boxed(item))
    }
}

/* Seems to be unused
const VECTOR_CONSTRUCTOR: ConstructorNumber = ConstructorNumber(0x1cb5c415);

impl<T> BoxedSerialize for Vec<T>
    where Self: BareSerialize
{
    fn serialize_boxed(&self) -> (ConstructorNumber, &dyn BareSerialize) {
        (VECTOR_CONSTRUCTOR, self)
    }
}

impl<T> BoxedDeserialize for Vec<T>
    where Self: BareDeserialize
{
    fn possible_constructors() -> Vec<ConstructorNumber> {
        vec![VECTOR_CONSTRUCTOR]
    }
    fn deserialize_boxed(id: ConstructorNumber, de: &mut Deserializer) -> Result<Self> {
        assert_eq!(id, VECTOR_CONSTRUCTOR);
        <Vec<T> as BareDeserialize>::deserialize_bare(de)
    }
}
*/

/// Vector of bytes
pub type bytes = Vec<u8>;

impl BareDeserialize for bytes {
    fn deserialize_bare(de: &mut Deserializer) -> Result<Self> {
        let len = de.read_u8()?;
        let (len, mut have_read) = match len {
            0..=253 => (len as usize, 1),
            254 => (de.read_u24::<LittleEndian>()? as usize, 4),
            255 => {
                let mut buf = [0u8; 7];
                de.read_exact(&mut buf)?;
                let len = (buf[0] as usize)
                    | (buf[1] as usize) << 8
                    | (buf[2] as usize) << 16
                    | (buf[3] as usize) << 24
                    | (buf[4] as usize) << 32
                    | (buf[5] as usize) << 40
                    | (buf[6] as usize) << 48;
                if len >= (1 << 32) {
                    fail!("Too big string of {} size is found", len)
                }
                (len, 8)
            }
        };
        if len > MAX_TL_BYTES_LEN {
            fail!("Too big length {len} of TL bytes")
        }
        let mut buf = Vec::new();
        buf.try_reserve_exact(len)
            .map_err(|e| error!("count {} is too big for {}: {}", len, type_name::<Self>(), e))?;
        buf.resize(len, 0);
        de.read_exact(&mut buf)?;
        have_read += len;
        let remainder = have_read % 4;
        if remainder != 0 {
            let mut buf = [0u8; 4];
            de.read_exact(&mut buf[remainder..])?;
        }
        Ok(buf)
    }
}

impl BareSerialize for bytes {
    fn constructor(&self) -> u32 {
        unreachable!()
    }
    fn serialize_bare(&self, ser: &mut Serializer) -> Result<()> {
        ser.write_bare::<[u8]>(self)
    }
}

impl BareSerialize for [u8] {
    fn constructor(&self) -> u32 {
        unreachable!()
    }

    fn serialize_bare(&self, ser: &mut Serializer) -> Result<()> {
        let len = self.len();
        let mut have_written = if len < 254 {
            ser.write_u8(len as u8)?;
            1
        } else if len < (1 << 24) {
            ser.write_u8(254)?;
            ser.write_u24::<LittleEndian>(len as u32)?;
            4
        } else if len < (1 << 32) {
            ser.write_u8(255)?;
            let buf = [
                (len & 0xFF) as u8,
                ((len >> 8) & 0xFF) as u8,
                ((len >> 16) & 0xFF) as u8,
                ((len >> 24) & 0xFF) as u8,
                ((len >> 32) & 0xFF) as u8,
                ((len >> 40) & 0xFF) as u8,
                ((len >> 48) & 0xFF) as u8,
            ];
            ser.write_all(&buf)?;
            8
        } else {
            fail!("String size of {} is too big to be stored", len)
        };
        ser.write_all(self)?;
        have_written += len;
        let remainder = have_written % 4;
        if remainder != 0 {
            let buf = [0u8; 4];
            ser.write_all(&buf[remainder..])?;
        }
        Ok(())
    }
}

macro_rules! impl_tl_primitive {
    ($tltype:ident, $ptype:ty, $read:ident, $write:ident) => {
        pub type $tltype = $ptype;

        impl BareDeserialize for $ptype {
            fn deserialize_bare(de: &mut Deserializer) -> Result<Self> {
                Ok(de.$read::<LittleEndian>()?)
            }
        }

        impl BareSerialize for $ptype {
            fn constructor(&self) -> u32 {
                unreachable!()
            }
            fn serialize_bare(&self, ser: &mut Serializer) -> Result<()> {
                ser.$write::<LittleEndian>(*self)?;
                Ok(())
            }
        }
    };
}

macro_rules! impl_tl_primitive_byte {
    ($tltype:ident, $ptype:ty, $read:ident, $write:ident) => {
        pub type $tltype = $ptype;

        impl BareDeserialize for $ptype {
            fn deserialize_bare(de: &mut Deserializer) -> Result<Self> {
                Ok(de.$read()?)
            }
        }

        impl BareSerialize for $ptype {
            fn constructor(&self) -> u32 {
                unreachable!()
            }
            fn serialize_bare(&self, ser: &mut Serializer) -> Result<()> {
                ser.$write(*self)?;
                Ok(())
            }
        }
    };
}

impl_tl_primitive_byte! { byte, u8, read_u8, write_u8 }
impl_tl_primitive! { int, i32, read_i32, write_i32 }
impl_tl_primitive! { uint, u32, read_u32, write_u32 }
impl_tl_primitive! { long, i64, read_i64, write_i64 }
impl_tl_primitive! { ulong, u64, read_u64, write_u64 }

pub type double = OrderedFloat<f64>;

impl BareDeserialize for double {
    fn deserialize_bare(de: &mut Deserializer) -> Result<Self> {
        Ok(de.read_f64::<LittleEndian>()?.into())
    }
}

impl BareSerialize for double {
    fn constructor(&self) -> u32 {
        unreachable!()
    }
    fn serialize_bare(&self, ser: &mut Serializer) -> Result<()> {
        ser.write_f64::<LittleEndian>(self.0)?;
        Ok(())
    }
}

// Built-in types:
//pub type int8 = i8;
pub type int32 = i32;
pub type int53 = i64;
pub type int64 = i64;
pub type uint8 = u8;
pub type uint32 = u32;
pub type uint64 = u64;

/// Flags built-in type.
pub type Flags = u32;
pub type lengthPrefixedTypedObject = LengthPrefixed<TypedObject>;
pub type True = bool;
/// String built-in type.
pub type string = String;
/// Alias of TLObject built-in type.
pub type TypedObject = TLObject;
/// Alias of TLObject built-in type.
pub type Object = TLObject;
pub type object = Object;
/// Function. Alias of TLObject built-in type.
pub type Function = TLObject;
pub type function = Function;
/// Alias of SecureBytes built-in type.
pub type secureBytes = crate::secure::SecureBytes;
/// Alias of SecureString built-in type.
pub type secureString = crate::secure::SecureString;

// Display traits

impl Display for crate::ton::overlay::node::Node {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        write!(
            f,
            "node id {}, overlay {}, version {}",
            self.id,
            base64_encode(self.overlay.as_slice()),
            self.version
        )
    }
}

impl Display for crate::ton::PublicKey {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        match self {
            crate::ton::PublicKey::Pub_Ed25519(key) => {
                write!(f, "{}", Ed25519KeyOption::from_public_key(key.key.as_array()).id())
            }
            x => write!(f, "{:?}", x),
        }
    }
}
