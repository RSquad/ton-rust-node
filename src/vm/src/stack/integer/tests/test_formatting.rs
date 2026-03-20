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
use super::*;

#[test]
fn test_formatting() {
    let value = IntegerData::from_u32(180149778);

    assert_eq!("180149778", value.to_str());
    assert_eq!("abcde12", value.to_str_radix(16));
    assert_eq!("1010101111001101111000010010", value.to_str_radix(2));

    assert_eq!(value.to_str(), format!("{}", value));
    assert_eq!(value.to_str_radix(16), format!("{:x}", value));
    assert_eq!(value.to_str_radix(16).to_uppercase(), format!("{:X}", value));
    assert_eq!(value.to_str_radix(2), format!("{:b}", value));
}
