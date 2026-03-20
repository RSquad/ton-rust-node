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
use super::IntegerData;
use num::Signed;
use std::fmt;

impl IntegerData {
    /// Converts value into String with given radix.
    pub fn to_str_radix(&self, radix: u32) -> String {
        match &self.value {
            None => "NaN".to_string(),
            Some(value) => value.to_str_radix(radix),
        }
    }

    /// Converts value to 0x hex notation, in case of negative number
    /// display - sign before 0x prefix as tonconter does
    pub fn to_str_hex(&self) -> String {
        match &self.value {
            None => "NaN".to_string(),
            Some(value) if value.is_negative() => {
                format!("-0x{}", value.abs().to_str_radix(16))
            }
            Some(value) => format!("0x{}", value.to_str_radix(16)),
        }
    }

    /// Converts value into String.
    pub fn to_str(&self) -> String {
        self.to_str_radix(10)
    }
}

impl fmt::Display for IntegerData {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.to_str())
    }
}

impl fmt::LowerHex for IntegerData {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.to_str_radix(16))
    }
}

impl fmt::UpperHex for IntegerData {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.to_str_radix(16).to_uppercase())
    }
}

impl fmt::Binary for IntegerData {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.to_str_radix(2))
    }
}
