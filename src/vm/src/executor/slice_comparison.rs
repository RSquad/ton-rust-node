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
use crate::{
    executor::{
        engine::{storage::fetch_stack, Engine},
        types::Instruction,
    },
    stack::{integer::IntegerData, StackItem},
};
use ton_block::{SliceData, Status};

fn unary<F>(engine: &mut Engine, name: &'static str, operation: F) -> Status
where
    F: Fn(&SliceData) -> StackItem,
{
    engine.load_instruction(Instruction::new(name))?;
    fetch_stack(engine, 1)?;
    let slice = engine.cmd.var(0).as_slice()?.clone();
    let r = operation(&slice);
    engine.cc.stack.push(r);
    Ok(())
}

fn binary<F>(engine: &mut Engine, name: &'static str, operation: F) -> Status
where
    F: Fn(SliceData, SliceData) -> StackItem,
{
    engine.load_instruction(Instruction::new(name))?;
    fetch_stack(engine, 2)?;
    let s0 = engine.cmd.var(0).as_slice()?.clone();
    let s1 = engine.cmd.var(1).as_slice()?.clone();
    let r = operation(s1, s0);
    engine.cc.stack.push(r);
    Ok(())
}

fn common_prefix<F>(engine: &mut Engine, name: &'static str, operation: F) -> Status
where
    F: Fn(Option<SliceData>, Option<SliceData>) -> StackItem,
{
    engine.load_instruction(Instruction::new(name))?;
    fetch_stack(engine, 2)?;
    let s0 = engine.cmd.var(0).as_slice()?;
    let s1 = engine.cmd.var(1).as_slice()?;
    let (_, r_s1, r_s0) = SliceData::common_prefix(s1, s0);
    let r = operation(r_s1, r_s0);
    engine.cc.stack.push(r);
    Ok(())
}

/// SEMPTY (s – s = ∅), checks whether a Slice s is empty
/// (i.e., contains no bits of data and no cell references).
pub(super) fn execute_sempty(engine: &mut Engine) -> Status {
    unary(engine, "SEMPTY", |slice| {
        boolean!((slice.remaining_bits() == 0) && (slice.remaining_references() == 0))
    })
}

/// SDEMPTY (s – s ≈ ∅), checks whether Slice s has no bits of data.
pub(super) fn execute_sdempty(engine: &mut Engine) -> Status {
    unary(engine, "SDEMPTY", |slice| boolean!(slice.remaining_bits() == 0))
}

/// SREMPTY (s – r(s) = 0), checks whether Slice s has no refer- ences.
pub(super) fn execute_srempty(engine: &mut Engine) -> Status {
    unary(engine, "SREMPTY", |slice| boolean!(slice.remaining_references() == 0))
}

/// SDFIRST (s – s0 = 1), checks whether the first bit of Slice s is a one.
pub(super) fn execute_sdfirst(engine: &mut Engine) -> Status {
    unary(engine, "SDFIRST", |slice| {
        boolean!((slice.remaining_bits() != 0) && (slice.get_bit_opt(0) == Some(true)))
    })
}

/// SDLEXCMP (s s′ – c), compares the data of s lexicographically
/// with the data of s′, returning −1, 0, or 1 depending on the result. s > s` => 1
pub(super) fn execute_sdlexcmp(engine: &mut Engine) -> Status {
    common_prefix(engine, "SDLEXCMP", |r_s1, r_s0| {
        int!(if r_s0.is_none() && r_s1.is_none() {
            0
        } else if r_s0.is_some() && r_s1.is_some() {
            if r_s1.unwrap().get_next_bit().unwrap() {
                1
            } else {
                -1
            }
        } else if r_s1.is_some() {
            1
        } else {
            -1
        })
    })
}

/// SDEQ(s s′ – s ≈ s′), checks whether the data parts of s and s′ coincide,
/// equivalent to SDLEXCMP; ISZERO.
pub(super) fn execute_sdeq(engine: &mut Engine) -> Status {
    common_prefix(engine, "SDEQ", |r_s1, r_s0| boolean!(r_s0.is_none() && r_s1.is_none()))
}

/// SDPFX (s s′ – ?), checks whether s is a prefix of s′.
pub(super) fn execute_sdpfx(engine: &mut Engine) -> Status {
    common_prefix(engine, "SDPFX", |r_s1, _| boolean!(r_s1.is_none()))
}

/// SDPFXREV (s s′ – ?), checks whether s′ is a prefix of s, equivalent
/// to SWAP; SDPFX.
pub(super) fn execute_sdpfxrev(engine: &mut Engine) -> Status {
    common_prefix(engine, "SDPFXREV", |_, r_s0| boolean!(r_s0.is_none()))
}

/// SDPPFX (s s′ – ?), checks whether s is a proper prefix of s′
/// (i.e., prefix distinct from s′).
pub(super) fn execute_sdppfx(engine: &mut Engine) -> Status {
    common_prefix(engine, "SDPPFX", |r_s1, r_s0| boolean!(r_s0.is_some() && r_s1.is_none()))
}

/// SDPPFXREV (s s′ – ?), checks whether s′ is a proper prefix of s.
pub(super) fn execute_sdppfxrev(engine: &mut Engine) -> Status {
    common_prefix(engine, "SDPPFXREV", |r_s1, r_s0| boolean!(r_s0.is_none() && r_s1.is_some()))
}

/// SDSFX(s s′ – ?), checks whether s is a suffix of s′.
pub(super) fn execute_sdsfx(engine: &mut Engine) -> Status {
    binary(engine, "SDSFX", |s1, mut s0| {
        boolean!({
            let l0 = s0.remaining_bits();
            let l1 = s1.remaining_bits();
            if l1 <= l0 {
                s0.shrink_data(l0 - l1..);
                let (_, r_s0, r_s1) = SliceData::common_prefix(&s0, &s1);
                r_s0.is_none() && r_s1.is_none()
            } else {
                false
            }
        })
    })
}

/// SDSFXREV (s s′ – ?), checks whether s′ is a suffix of s.
pub(super) fn execute_sdsfxrev(engine: &mut Engine) -> Status {
    binary(engine, "SDSFXREV", |mut s1, s0| {
        boolean!({
            let l0 = s0.remaining_bits();
            let l1 = s1.remaining_bits();
            if l0 <= l1 {
                s1.shrink_data(l1 - l0..);
                let (_, r_s0, r_s1) = SliceData::common_prefix(&s0, &s1);
                r_s0.is_none() && r_s1.is_none()
            } else {
                false
            }
        })
    })
}

///  SDPSFX (s s′ – ?), checks whether s is a proper suffix of s′.
pub(super) fn execute_sdpsfx(engine: &mut Engine) -> Status {
    binary(engine, "SDPSFX", |s1, mut s0| {
        boolean!({
            let l0 = s0.remaining_bits();
            let l1 = s1.remaining_bits();
            if l1 < l0 {
                s0.shrink_data(l0 - l1..);
                let (_, r_s0, r_s1) = SliceData::common_prefix(&s0, &s1);
                r_s0.is_none() && r_s1.is_none()
            } else {
                false
            }
        })
    })
}

/// SDPSFXREV (s s′ – ?), checks whether s′ is a proper suffix of s.
pub(super) fn execute_sdpsfxrev(engine: &mut Engine) -> Status {
    binary(engine, "SDPSFXREV", |mut s1, s0| {
        boolean!({
            let l0 = s0.remaining_bits();
            let l1 = s1.remaining_bits();
            if l0 < l1 {
                s1.shrink_data(l1 - l0..);
                let (_, r_s0, r_s1) = SliceData::common_prefix(&s0, &s1);
                r_s0.is_none() && r_s1.is_none()
            } else {
                false
            }
        })
    })
}

/// SDCNTLEAD0 (s – n), returns the number of leading zeroes in s.
pub(super) fn execute_sdcntlead0(engine: &mut Engine) -> Status {
    unary(engine, "SDCNTLEAD0", |slice| {
        int!({
            let n = slice.remaining_bits();
            (0..n).position(|i| slice.get_bit_opt(i) == Some(true)).unwrap_or(n)
        })
    })
}

/// SDCNTLEAD1 (s – n), returns the number of leading ones in s.
pub(super) fn execute_sdcntlead1(engine: &mut Engine) -> Status {
    unary(engine, "SDCNTLEAD1", |slice| {
        int!({
            let n = slice.remaining_bits();
            (0..n).position(|i| slice.get_bit_opt(i) == Some(false)).unwrap_or(n)
        })
    })
}

/// SDCNTTRAIL0 (s – n), returns the number of trailing zeroes in s.
pub(super) fn execute_sdcnttrail0(engine: &mut Engine) -> Status {
    unary(engine, "SDCNTTRAIL0", |slice| {
        int!({
            let n = slice.remaining_bits();
            (0..n).position(|i| slice.get_bit_opt(n - i - 1) == Some(true)).unwrap_or(n)
        })
    })
}

/// SDCNTTRAIL1 (s – n), returns the number of trailing ones in s.
pub(super) fn execute_sdcnttrail1(engine: &mut Engine) -> Status {
    unary(engine, "SDCNTTRAIL1", |slice| {
        int!({
            let n = slice.remaining_bits();
            (0..n).position(|i| slice.get_bit_opt(n - i - 1) == Some(false)).unwrap_or(n)
        })
    })
}
