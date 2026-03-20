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
use super::errors::ParameterError;
use num::Num;
use std::{
    cmp::PartialOrd,
    ops::{Bound, Range, RangeBounds},
};

fn parse_range<T, R>(range: R) -> impl Fn(&str) -> Result<T, ParameterError>
where
    T: Num + PartialOrd,
    R: RangeBounds<T>,
{
    move |p: &str| match T::from_str_radix(p, 10) {
        Ok(value) => {
            match range.start_bound() {
                Bound::Included(min) => {
                    if value < *min {
                        return Err(ParameterError::OutOfRange);
                    }
                }
                Bound::Excluded(min_excluded) => {
                    if value <= *min_excluded {
                        return Err(ParameterError::OutOfRange);
                    }
                }
                Bound::Unbounded => {}
            }
            match range.end_bound() {
                Bound::Included(max) => {
                    if value > *max {
                        return Err(ParameterError::OutOfRange);
                    }
                }
                Bound::Excluded(max_excluded) => {
                    if value >= *max_excluded {
                        return Err(ParameterError::OutOfRange);
                    }
                }
                Bound::Unbounded => {}
            }
            Ok(value)
        }
        _ => Err(ParameterError::UnexpectedType),
    }
}

pub(super) fn parse_const_u2(par: &str) -> Result<u8, ParameterError> {
    parse_range(0..4)(par)
}

pub(super) fn parse_const_i4(par: &str) -> Result<u8, ParameterError> {
    parse_range(-1i8..=14)(par).map(|e| (e & 0x0F) as u8)
}

pub(super) fn parse_const_u4(par: &str) -> Result<u8, ParameterError> {
    parse_range(0u8..=15)(par)
}

pub(super) fn parse_const_u4_plus_one(par: &str) -> Result<u8, ParameterError> {
    parse_range(1u8..=16)(par).map(|e| e - 1)
}

pub(super) fn parse_const_u4_plus_two(par: &str) -> Result<u8, ParameterError> {
    parse_range(2u8..=17)(par).map(|e| e - 2)
}

pub(super) fn parse_const_u4_14(par: &str) -> Result<u8, ParameterError> {
    parse_range(0i8..=14)(par).map(|e| e as u8)
}

pub(super) fn parse_const_u4_1_14(par: &str) -> Result<u8, ParameterError> {
    parse_range(1i8..=14)(par).map(|e| e as u8)
}

pub(super) fn parse_const_u4_nonzero(par: &str) -> Result<u8, ParameterError> {
    parse_range(1u8..=16)(par)
}

// 5-bit arguments
pub(super) fn parse_const_u5(par: &str) -> Result<u8, ParameterError> {
    parse_range(0u8..32)(par)
}

// 10-bit arguments
pub(super) fn parse_const_u10(par: &str) -> Result<u16, ParameterError> {
    parse_range(0..1024)(par)
}

// 11-bit arguments for THROW* instructions
pub(super) fn parse_const_u11(par: &str) -> Result<u16, ParameterError> {
    parse_range(0u16..2048)(par)
}

// 12-bit arguments for RUNVM instructions
pub(super) fn parse_const_u12(par: &str) -> Result<u16, ParameterError> {
    parse_range(0..=0xFFFu16)(par)
}

pub(super) fn parse_const_u14(par: &str) -> Result<u16, ParameterError> {
    parse_range(0u16..16384)(par)
}

/// parses as argument for SETCP -15..240
pub(super) fn parse_const_u8_setcp(par: &str) -> Result<u8, ParameterError> {
    parse_range(-15..240)(par).map(|z| z as u8)
}

pub(super) fn parse_const_i8(par: &str) -> Result<u8, ParameterError> {
    parse_range(-128i16..=127)(par).map(|e| e as u8)
}

pub(super) fn parse_const_u8(par: &str) -> Result<u8, ParameterError> {
    parse_range(0..=255u8)(par)
}

pub(super) fn parse_const_u8_0_4(par: &str) -> Result<u8, ParameterError> {
    parse_range(0..=4u8)(par)
}

pub(super) fn parse_const_u8_plus_one(par: &str) -> Result<u8, ParameterError> {
    parse_range(1u16..=256)(par).map(|e| (e - 1) as u8)
}

pub(super) fn parse_const_u8_240(par: &str) -> Result<u8, ParameterError> {
    parse_range(0u8..240)(par)
}

pub(super) fn parse_control_register(par: &str) -> Result<u8, ParameterError> {
    Ok(parse_register(par, 'C', 0..16)? as u8)
}

pub(super) fn parse_register(
    register: &str,
    symbol: char,
    range: Range<isize>,
) -> Result<isize, ParameterError> {
    if register.len() <= 1 || register.chars().next().unwrap().to_ascii_uppercase() != symbol {
        Err(ParameterError::UnexpectedType)
    } else {
        match register[1..].parse::<isize>() {
            Ok(number) => {
                if (number < range.start) || (number >= range.end) {
                    Err(ParameterError::OutOfRange)
                } else {
                    Ok(number)
                }
            }
            Err(_e) => Err(ParameterError::UnexpectedType),
        }
    }
}

pub fn parse_slice(slice: &str, bits: usize) -> Result<Vec<u8>, ParameterError> {
    if !slice.chars().next().unwrap().eq_ignore_ascii_case(&'X') {
        log::error!(target: "compile", "base not set");
        Err(ParameterError::UnexpectedType)
    } else {
        parse_slice_base(&slice[1..], bits, 16)
    }
}

pub fn parse_slice_base(
    slice: &str,
    mut bits: usize,
    base: u32,
) -> Result<Vec<u8>, ParameterError> {
    debug_assert!(bits < 8, "it is offset to get slice parsed");
    let origin_bits = bits;
    let mut acc = 0u8;
    let mut data = vec![];
    let mut completion_tag = false;
    for ch in slice.chars() {
        if completion_tag {
            return Err(ParameterError::UnexpectedType);
        }
        match ch.to_digit(base) {
            Some(x) => {
                if bits < 4 {
                    acc |= (x << (4 - bits)) as u8;
                    bits += 4;
                } else {
                    acc |= x as u8 >> (bits - 4);
                    data.push(acc);
                    acc = (x << (12 - bits)) as u8;
                    bits -= 4;
                }
            }
            None => {
                if ch == '_' {
                    completion_tag = true
                } else {
                    return Err(ParameterError::UnexpectedType);
                }
            }
        }
    }

    let mut removing_trailing_zeroes = || {
        while data.last() == Some(&0) {
            data.pop();
        }
        if data.is_empty() {
            data.push(1 << (7 - origin_bits));
        }
    };

    if bits != 0 {
        if completion_tag {
            if acc == 0 {
                removing_trailing_zeroes();
            }
        } else {
            acc |= 1 << (7 - bits);
        }
        if acc != 0 || data.is_empty() {
            data.push(acc);
        }
    } else if completion_tag {
        removing_trailing_zeroes();
    } else {
        data.push(0x80);
    }
    Ok(data)
}

pub(super) fn parse_stack_register_u4(par: &str) -> Result<u8, ParameterError> {
    Ok(parse_register(par, 'S', 0..16)? as u8)
}

pub(super) fn parse_stack_register_u4_minus_one(par: &str) -> Result<u8, ParameterError> {
    Ok((parse_register(par, 'S', -1..15)? + 1) as u8)
}

pub(super) fn parse_stack_register_u4_minus_two(par: &str) -> Result<u8, ParameterError> {
    Ok((parse_register(par, 'S', -2..14)? + 2) as u8)
}

pub(super) fn parse_plduz_parameter(par: &str) -> Result<u8, ParameterError> {
    (parse_range(32u16..=256))(par).and_then(|c| {
        if c % 32 == 0 {
            Ok(((c / 32) - 1) as u8)
        } else {
            Err(ParameterError::OutOfRange)
        }
    })
}

pub(super) fn parse_string(arg: &str) -> Vec<u8> {
    let mut string = String::from(arg);
    if string.to_ascii_uppercase().starts_with('X') {
        string.remove(0);
        let res = hex::decode(string);
        if let Ok(res) = res {
            return res;
        }
    }
    Vec::from(arg)
}
