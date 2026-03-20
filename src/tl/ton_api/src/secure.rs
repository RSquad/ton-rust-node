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
use crate::{BareDeserialize, BareSerialize};
use std::{
    fmt::{Debug, Display, Formatter},
    hash::{Hash, Hasher},
};

/// SecureBytes built-in type.
#[derive(Clone, PartialEq)]
pub struct SecureBytes(secstr::SecVec<u8>);

impl SecureBytes {
    pub fn new(cont: Vec<u8>) -> Self {
        Self(secstr::SecVec::<u8>::new(cont))
    }

    /// Borrow the contents of the string.
    pub fn unsecure(&self) -> &[u8] {
        self.0.unsecure()
    }

    /// Mutably borrow the contents of the string.
    pub fn unsecure_mut(&mut self) -> &mut [u8] {
        self.0.unsecure_mut()
    }

    /// Overwrite the string with zeros. This is automatically called in the destructor.
    pub fn zero_out(&mut self) {
        self.0.zero_out()
    }
}

impl Default for SecureBytes {
    fn default() -> Self {
        Self::new(Vec::default())
    }
}

impl Debug for SecureBytes {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        Debug::fmt(&self.0, f)
    }
}

impl Display for SecureBytes {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl Hash for SecureBytes {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.unsecure().hash(state);
    }
}

impl BareSerialize for SecureBytes {
    fn constructor(&self) -> u32 {
        unreachable!()
    }
    fn serialize_bare(&self, ser: &mut crate::Serializer) -> crate::Result<()> {
        self.0.unsecure().serialize_bare(ser)
    }
}

impl BareDeserialize for SecureBytes {
    fn deserialize_bare(de: &mut crate::Deserializer) -> crate::Result<Self> {
        Vec::<u8>::deserialize_bare(de).map(|vec| SecureBytes(secstr::SecVec::<u8>::new(vec)))
    }
}

/// SecureString built-in type.
pub type SecureString = SecureBytes;
