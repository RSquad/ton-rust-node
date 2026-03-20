/*
 * Copyright (C) 2019-2024 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::stack::StackItem;
use std::fmt;
use ton_block::{fail, ExceptionCode, Result};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SaveList {
    storage: [Option<StackItem>; Self::NUMREGS],
}

impl SaveList {
    pub const NUMREGS: usize = 7;
    pub const REGS: [usize; Self::NUMREGS] = [0, 1, 2, 3, 4, 5, 7];
    const fn adjust(index: usize) -> usize {
        if index == 7 {
            6
        } else {
            index
        }
    }

    pub const fn new() -> Self {
        Self { storage: [None, None, None, None, None, None, None] }
    }
    pub fn can_put(index: usize, value: &StackItem) -> bool {
        match index {
            0 | 1 | 3 => value.as_continuation().is_ok(),
            2 => value.as_continuation().is_ok() || value.is_null(),
            4 | 5 => value.as_cell().is_ok(),
            7 => value.as_tuple().is_ok(),
            _ => false,
        }
    }
    pub fn check_can_put(index: usize, value: &StackItem) -> Result<()> {
        if Self::can_put(index, value) {
            Ok(())
        } else {
            fail!(ExceptionCode::TypeCheckError, "wrong item {} for index {}", value, index)
        }
    }
    pub fn get(&self, index: usize) -> Option<&StackItem> {
        self.storage[Self::adjust(index)].as_ref()
    }
    pub fn get_mut(&mut self, index: usize) -> Option<&mut StackItem> {
        self.storage[Self::adjust(index)].as_mut()
    }
    pub fn is_empty(&self) -> bool {
        for v in &self.storage {
            if v.is_some() {
                return false;
            }
        }
        true
    }
    pub fn put(&mut self, index: usize, value: StackItem) -> Result<Option<StackItem>> {
        Self::check_can_put(index, &value)?;
        Ok(self.put_opt(index, value))
    }
    pub fn put_opt(&mut self, index: usize, value: StackItem) -> Option<StackItem> {
        debug_assert!(Self::can_put(index, &value));
        self.storage[Self::adjust(index)].replace(value)
    }
    pub fn apply(&mut self, other: &mut Self) {
        for index in 0..Self::NUMREGS {
            if other.storage[index].is_some() {
                self.storage[index] = std::mem::take(&mut other.storage[index]);
            }
        }
    }
    pub fn remove(&mut self, index: usize) -> Option<StackItem> {
        std::mem::take(&mut self.storage[Self::adjust(index)])
    }
}

impl fmt::Display for SaveList {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "--- Control registers ------------------")?;
        for i in 0..Self::NUMREGS {
            if let Some(item) = &self.storage[i] {
                writeln!(f, "{}: {}", i, item)?
            }
        }
        writeln!(f, "{:-<40}", "")
    }
}
