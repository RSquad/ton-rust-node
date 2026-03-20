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
        gas::gas_state::Gas,
        types::Instruction,
    },
    stack::{
        integer::{behavior::Signaling, math::Round, IntegerData},
        StackItem,
    },
};
use std::{
    str::FromStr,
    sync::{Arc, LazyLock},
};
use ton_block::{
    aggregate_and_verify, aggregate_public_keys_and_verify, aggregate_pure_bls_signatures, error,
    fail, g1_add, g1_in_group, g1_mul, g1_multiexp, g1_neg, g1_sub, g1_zero, g2_add, g2_in_group,
    g2_mul, g2_multiexp, g2_neg, g2_sub, g2_zero, map_to_g1, map_to_g2, pairing, verify,
    ExceptionCode, Result, SliceData, Status, BLS_G1_LEN, BLS_G2_LEN, BLS_PUBLIC_KEY_LEN,
    BLS_SCALAR_LEN, BLS_SIG_LEN,
};

// G1-points and public keys: 48-byte slice.
// G2-points and signatures: 96-byte slice.
// Elements of field FP: 48-byte slice.
// Elements of field FP2: 96-byte slice.
// Messages: slice. Number of bits should be divisible by 8.

//
// Utils
//

static R: LazyLock<Arc<IntegerData>> = LazyLock::new(|| {
    Arc::new(
        IntegerData::from_str(
            "52435875175126190479447740508185965837690552500527637822603658699938581184513",
        )
        .expect("Wrong blst R value string"),
    )
});

fn slice_to_msg(slice: &SliceData) -> Result<Vec<u8>> {
    if !slice.remaining_bits().is_multiple_of(8) {
        fail!(
            ExceptionCode::CellUnderflow,
            "message does not consist of an integer number of bytes"
        )
    } else {
        Ok(slice.get_bytestring(0))
    }
}

//
// High-level operations
//

/// BLS_VERIFY ( pk msg sgn - bool)
/// Checks BLS signature, return true on success, false otherwise.
pub(super) fn execute_bls_verify(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("BLS_VERIFY"))?;

    fetch_stack(engine, 3)?;

    engine.try_use_gas(Gas::bls_verify_gas_price())?;

    let sgn = engine.cmd.var(0).as_slice()?.clone().get_next_bytes(BLS_SIG_LEN)?;
    let msg = slice_to_msg(engine.cmd.var(1).as_slice()?)?;
    let pk = engine.cmd.var(2).as_slice()?.clone().get_next_bytes(BLS_PUBLIC_KEY_LEN)?;

    let res = verify(sgn.as_slice().try_into()?, &msg, pk.as_slice().try_into()?);

    engine.cc.stack.push(boolean!(res.unwrap_or(false)));

    Ok(())
}

/// BLS_AGGREGATE (sig_1 ... sig_n n - sig)
/// Aggregates signatures. `n>0`.
/// Throw exception if `n=0` or if some `sig_i` is not a valid signature.
pub(super) fn execute_bls_aggregate(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("BLS_AGGREGATE"))?;

    fetch_stack(engine, 1)?;
    let n = engine.cmd.var(0).as_stack_index()?;
    if n == 0 || n > engine.stack().depth() {
        fail!(ExceptionCode::RangeCheckError);
    }
    engine.try_use_gas(Gas::bls_aggregate_gas_price(n as i64))?;

    fetch_stack(engine, n)?;
    let mut signs = Vec::with_capacity(n);
    for i in 1..=n {
        signs.push(engine.cmd.var(i).as_slice()?.clone().get_next_bytes(BLS_SIG_LEN)?)
    }
    let mut signs_refs = Vec::<&[u8; BLS_SIG_LEN]>::with_capacity(n);
    for sign in &signs {
        signs_refs.push(sign.as_slice().try_into()?);
    }

    let res = aggregate_pure_bls_signatures(&signs_refs)?;
    engine
        .cc
        .stack
        .push(StackItem::Slice(SliceData::with_bitstring(res.as_slice(), BLS_SIG_LEN * 8)));

    Ok(())
}

/// BLS_FASTAGGREGATEVERIFY (pk_1 ... pk_n n msg sig - bool)
/// Checks aggregated BLS signature for keys `pk_1...pk_n` and message `msg`.
/// Return true on success, false otherwise. Return false if `n=0`
pub(super) fn execute_bls_fast_aggregate_verify(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("BLS_FASTAGGREGATEVERIFY"))?;

    fetch_stack(engine, 3)?;
    let sgn = engine.cmd.var(0).as_slice()?.clone().get_next_bytes(BLS_SIG_LEN)?;
    let msg = slice_to_msg(engine.cmd.var(1).as_slice()?)?;
    let n = engine.cmd.var(2).as_stack_index()?;
    if n > engine.stack().depth() {
        fail!(ExceptionCode::RangeCheckError);
    }
    engine.try_use_gas(Gas::bls_fastaggregateverify_gas_price(n as i64))?;
    let res = if n == 0 {
        false
    } else {
        fetch_stack(engine, n)?;
        let mut pks = Vec::with_capacity(n);
        for i in 3..n + 3 {
            pks.push(engine.cmd.var(i).as_slice()?.clone().get_next_bytes(BLS_PUBLIC_KEY_LEN)?)
        }
        let mut pks_refs = Vec::<&[u8; BLS_PUBLIC_KEY_LEN]>::with_capacity(n);
        for pk in &pks {
            pks_refs.push(pk.as_slice().try_into()?);
        }

        aggregate_public_keys_and_verify(sgn.as_slice().try_into()?, &msg, &pks_refs)
            .unwrap_or(false)
    };
    engine.cc.stack.push(boolean!(res));

    Ok(())
}

/// BLS_AGGREGATEVERIFY ( pk_1 msg_1 ... pk_n msg_n n sgn - bool)
/// Checks aggregated BLS signature for key-message pairs `pk_1 msg_1...pk_n msg_n`.
/// Return true on success, false otherwise. Return false if `n=0`
pub(super) fn execute_bls_aggregate_verify(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("BLS_AGGREGATEVERIFY"))?;

    fetch_stack(engine, 2)?;
    let sgn = engine.cmd.var(0).as_slice()?.clone().get_next_bytes(BLS_SIG_LEN)?;
    let n = engine.cmd.var(1).as_stack_index()?;
    if n * 2 > engine.stack().depth() {
        fail!(ExceptionCode::RangeCheckError);
    }
    engine.try_use_gas(Gas::bls_aggregateverify_gas_price(n as i64))?;
    let res = if n == 0 {
        false
    } else {
        fetch_stack(engine, n * 2)?;
        let mut pks = Vec::with_capacity(n);
        let mut msgs = Vec::with_capacity(n);
        for i in 0..n {
            msgs.push(slice_to_msg(engine.cmd.var(2 + i * 2).as_slice()?)?);
            pks.push(
                engine.cmd.var(3 + i * 2).as_slice()?.clone().get_next_bytes(BLS_PUBLIC_KEY_LEN)?,
            );
        }
        let msgs_refs: Vec<&[u8]> = msgs.iter().map(|sig| &sig[..]).collect();
        let mut pks_refs = Vec::<&[u8; BLS_PUBLIC_KEY_LEN]>::with_capacity(n);
        for pk in &pks {
            pks_refs.push(pk.as_slice().try_into()?);
        }

        aggregate_and_verify(sgn.as_slice().try_into()?, &msgs_refs, &pks_refs).unwrap_or(false)
    };
    engine.cc.stack.push(boolean!(res));

    Ok(())
}

//
// Low-level operations
//

// Generics

pub(super) fn bls_generic_add_sub<P, const L: usize>(
    engine: &mut Engine,
    instruction: &'static str,
    gas: i64,
    op: P,
) -> Status
where
    P: FnOnce(&[u8; L], &[u8; L]) -> Result<[u8; L]>,
{
    engine.load_instruction(Instruction::new(instruction))?;
    engine.try_use_gas(gas)?;

    fetch_stack(engine, 2)?;
    let y = engine.cmd.var(0).as_slice()?.clone().get_next_bytes(L)?;
    let x = engine.cmd.var(1).as_slice()?.clone().get_next_bytes(L)?;

    let res = op(x.as_slice().try_into()?, y.as_slice().try_into()?)
        .map_err(|err| error!(ExceptionCode::UnknownError, "{}", err))?;

    engine.cc.stack.push(StackItem::Slice(SliceData::with_bitstring(res.as_slice(), L * 8)));

    Ok(())
}

pub(super) fn bls_generic_mul<P, const L: usize>(
    engine: &mut Engine,
    instruction: &'static str,
    gas: i64,
    op: P,
) -> Status
where
    P: FnOnce(&[u8; L], &[u8; BLS_SCALAR_LEN]) -> Result<[u8; L]>,
{
    engine.load_instruction(Instruction::new(instruction))?;
    engine.try_use_gas(gas)?;

    fetch_stack(engine, 2)?;
    let s = engine.cmd.var(0).as_integer()?;
    let x = engine.cmd.var(1).as_slice()?.clone().get_next_bytes(L)?;

    // Using FloorToNegativeInfinity we got positive remainder anyway,
    // it is what we need because blst scalar is always positive
    let (_, s) = s.div::<Signaling>(&R, Round::FloorToNegativeInfinity)?;

    let res = op(
        x.as_slice().try_into()?,
        s.as_builder(BLS_SCALAR_LEN * 8, false, true)?.data().try_into()?,
    )
    .map_err(|err| error!(ExceptionCode::UnknownError, "{}", err))?;

    engine.cc.stack.push(StackItem::Slice(SliceData::with_bitstring(res.as_slice(), L * 8)));

    Ok(())
}

pub(super) fn bls_generic_multiexp<P, G, const L: usize>(
    engine: &mut Engine,
    instruction: &'static str,
    count_gas: G,
    op: P,
) -> Status
where
    P: FnOnce(&[&[u8; L]], &[&[u8; BLS_SCALAR_LEN]]) -> Result<[u8; L]>,
    G: Fn(i64) -> i64,
{
    engine.load_instruction(Instruction::new(instruction))?;

    fetch_stack(engine, 1)?;

    let n = engine.cmd.var(0).as_stack_index()?;
    if n * 2 > engine.stack().depth() {
        fail!(ExceptionCode::RangeCheckError);
    }

    engine.try_use_gas(count_gas(n as i64))?;

    fetch_stack(engine, n * 2)?;
    let mut points = Vec::with_capacity(n);
    let mut scalars = Vec::<Vec<u8>>::with_capacity(n);
    for i in 0..n {
        let s = engine.cmd.var(1 + i * 2).as_integer()?;
        let (_, s) = s.div::<Signaling>(&R, Round::FloorToNegativeInfinity)?;
        scalars.push(s.as_vec(BLS_SCALAR_LEN * 8, false, false)?);

        points.push(engine.cmd.var(2 + i * 2).as_slice()?.clone().get_next_bytes(L)?);
    }
    let mut points_refs = Vec::<&[u8; L]>::with_capacity(n);
    for point in &points {
        points_refs.push(point.as_slice().try_into()?);
    }
    let mut scalars_refs = Vec::<&[u8; BLS_SCALAR_LEN]>::with_capacity(n);
    for scalar in &scalars {
        scalars_refs.push(scalar.as_slice().try_into()?);
    }

    let res = op(&points_refs, &scalars_refs)
        .map_err(|err| error!(ExceptionCode::UnknownError, "{}", err))?;

    engine.cc.stack.push(StackItem::Slice(SliceData::with_bitstring(res.as_slice(), L * 8)));

    Ok(())
}

pub(super) fn bls_generic_map<P, const L: usize>(
    engine: &mut Engine,
    instruction: &'static str,
    gas: i64,
    op: P,
) -> Status
where
    P: (FnOnce(&[u8; L]) -> Result<[u8; L]>),
{
    engine.load_instruction(Instruction::new(instruction))?;
    engine.try_use_gas(gas)?;

    fetch_stack(engine, 1)?;
    let x = engine.cmd.var(0).as_slice()?.clone().get_next_bytes(L)?;

    let res = op(x.as_slice().try_into()?)
        .map_err(|err| error!(ExceptionCode::UnknownError, "{}", err))?;

    engine.cc.stack.push(StackItem::Slice(SliceData::with_bitstring(res.as_slice(), L * 8)));

    Ok(())
}

pub(super) fn bls_generic_iszero<P, const L: usize>(
    engine: &mut Engine,
    instruction: &'static str,
    op: P,
) -> Status
where
    P: (FnOnce() -> [u8; L]),
{
    engine.load_instruction(Instruction::new(instruction))?;

    fetch_stack(engine, 1)?;
    let x = engine.cmd.var(0).as_slice()?.clone().get_next_bytes(L)?;
    let zero = op();

    engine.cc.stack.push(boolean!(x == zero));

    Ok(())
}

pub(super) fn bls_generic_in_group<P, const L: usize>(
    engine: &mut Engine,
    instruction: &'static str,
    gas: i64,
    op: P,
) -> Status
where
    P: (FnOnce(&[u8; L]) -> bool),
{
    engine.load_instruction(Instruction::new(instruction))?;
    engine.try_use_gas(gas)?;

    fetch_stack(engine, 1)?;
    let x = engine.cmd.var(0).as_slice()?.clone().get_next_bytes(L)?;
    let res = op(x.as_slice().try_into()?);

    engine.cc.stack.push(boolean!(res));

    Ok(())
}

// G1

/// BLS_G1_ADD ( x y - x+y)
/// Addition on G1.
pub(super) fn execute_bls_g1_add(engine: &mut Engine) -> Status {
    bls_generic_add_sub(engine, "BLS_G1_ADD", Gas::bls_g1_add_sub_gas_price(), g1_add)
}

/// BLS_G1_SUB ( x y - x-y)
/// Subtraction on G1.
pub(super) fn execute_bls_g1_sub(engine: &mut Engine) -> Status {
    bls_generic_add_sub(engine, "BLS_G1_SUB", Gas::bls_g1_add_sub_gas_price(), g1_sub)
}

/// BLS_G1_NEG ( x - -x)
/// Negation on G1.
pub(super) fn execute_bls_g1_neg(engine: &mut Engine) -> Status {
    bls_generic_map(engine, "BLS_G1_NEG", Gas::bls_g1_neg_gas_price(), g1_neg)
}

/// BLS_G1_MUL ( x s - x*s)
/// Multiplies G1 point `x` by scalar `s`. Any `s` is valid, including negative.
pub(super) fn execute_bls_g1_mul(engine: &mut Engine) -> Status {
    bls_generic_mul(engine, "BLS_G1_MUL", Gas::bls_g1_mul_gas_price(), g1_mul)
}

/// BLS_G1_MULTIEXP ( x_1 s_1 ... x_n s_n n - x_1*s_1+...+x_n*s_n)
/// Calculates `x_1*s_1+...+x_n*s_n` for G1 points `x_i` and scalars `s_i`.
/// Returns zero point if `n=0`. Any `s_i` is valid, including negative.
pub(super) fn execute_bls_g1_multiexp(engine: &mut Engine) -> Status {
    bls_generic_multiexp(engine, "BLS_G1_MULTIEXP", Gas::bls_g1_multiexp_gas_price, g1_multiexp)
}

/// BLS_G1_ZERO ( - zero)
/// Pushes zero point in G1.
pub(super) fn execute_g1_zero(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("BLS_G1_ZERO"))?;
    engine
        .cc
        .stack
        .push(StackItem::Slice(SliceData::with_bitstring(g1_zero().as_slice(), BLS_G1_LEN * 8)));
    Ok(())
}

/// BLS_MAP_TO_G1 ( f - x)
/// Converts FP element `f` to a G1 point.
pub(super) fn execute_bls_map_to_g1(engine: &mut Engine) -> Status {
    bls_generic_map(engine, "BLS_MAP_TO_G1", Gas::bls_map_to_g1_gas_price(), |p| Ok(map_to_g1(p)))
}

/// BLS_G1_INGROUP ( x - bool)
/// Checks that slice `x` represents a valid element of G1.
pub(super) fn execute_bls_g1_ingroup(engine: &mut Engine) -> Status {
    bls_generic_in_group(engine, "BLS_G1_INGROUP", Gas::bls_g1_ingroup_gas_price(), g1_in_group)
}

/// BLS_G1_ISZERO ( x - bool)
/// Checks that G1 point `x` is equal to zero.
pub(super) fn execute_bls_g1_iszero(engine: &mut Engine) -> Status {
    bls_generic_iszero(engine, "BLS_G1_ISZERO", g1_zero)
}

// G2

/// BLS_G2_ADD ( x y - x+y)
/// Addition on G2.
pub(super) fn execute_bls_g2_add(engine: &mut Engine) -> Status {
    bls_generic_add_sub(engine, "BLS_G2_ADD", Gas::bls_g2_add_sub_gas_price(), g2_add)
}

/// BLS_G2_SUB ( x y - x-y)
/// Subtraction on G2.
pub(super) fn execute_bls_g2_sub(engine: &mut Engine) -> Status {
    bls_generic_add_sub(engine, "BLS_G2_SUB", Gas::bls_g2_add_sub_gas_price(), g2_sub)
}

/// BLS_G2_NEG ( x - -x)
/// Negation on G2.
pub(super) fn execute_bls_g2_neg(engine: &mut Engine) -> Status {
    bls_generic_map(engine, "BLS_G2_NEG", Gas::bls_g2_neg_gas_price(), g2_neg)
}

/// BLS_G2_MUL ( x s - x*s)
/// Multiplies G2 point `x` by scalar `s`.<br/>Any `s` is valid, including negative.
pub(super) fn execute_bls_g2_mul(engine: &mut Engine) -> Status {
    bls_generic_mul(engine, "BLS_G2_MUL", Gas::bls_g2_mul_gas_price(), g2_mul)
}

/// BLS_G2_MULTIEXP ( x_1 s_1 ... x_n s_n n - x_1*s_1+...+x_n*s_n)
/// Calculates `x_1*s_1+...+x_n*s_n` for G2 points `x_i` and scalars `s_i`.
/// Returns zero point if `n=0`. Any `s_i` is valid, including negative.
pub(super) fn execute_bls_g2_multiexp(engine: &mut Engine) -> Status {
    bls_generic_multiexp(engine, "BLS_G2_MULTIEXP", Gas::bls_g2_multiexp_gas_price, g2_multiexp)
}

/// BLS_G2_ZERO ( - zero)
/// Pushes zero point in G2.
pub(super) fn execute_g2_zero(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("BLS_G2_ZERO"))?;
    engine
        .cc
        .stack
        .push(StackItem::Slice(SliceData::with_bitstring(g2_zero().as_slice(), BLS_G2_LEN * 8)));

    Ok(())
}

/// BLS_MAP_TO_G2 ( f - x)
/// Converts FP2 element `f` to a G2 point.
pub(super) fn execute_bls_map_to_g2(engine: &mut Engine) -> Status {
    bls_generic_map(engine, "BLS_MAP_TO_G2", Gas::bls_map_to_g2_gas_price(), |p| Ok(map_to_g2(p)))
}

/// BLS_G2_INGROUP ( x - bool)
/// Checks that slice `x` represents a valid element of G2.
pub(super) fn execute_bls_g2_ingroup(engine: &mut Engine) -> Status {
    bls_generic_in_group(engine, "BLS_G2_INGROUP", Gas::bls_g2_ingroup_gas_price(), g2_in_group)
}

/// BLS_G2_ISZERO ( x - bool)
/// Checks that G2 point `x` is equal to zero.
pub(super) fn execute_bls_g2_iszero(engine: &mut Engine) -> Status {
    bls_generic_iszero(engine, "BLS_G2_ISZERO", g2_zero)
}

// Misc

/// BLS_PAIRING ( x_1 y_1 ... x_n y_n n - bool)
/// Given G1 points `x_i` and G2 points `y_i`, calculates and multiply pairings of `x_i, y_i`.
/// Returns true if the result is the multiplicative identity in FP12, false otherwise.
/// Returns false if `n=0`.
pub(super) fn execute_bls_pairing(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("BLS_PAIRING"))?;

    fetch_stack(engine, 1)?;

    let n = engine.cmd.var(0).as_stack_index()?;
    if n * 2 > engine.stack().depth() {
        fail!(ExceptionCode::RangeCheckError);
    }

    engine.try_use_gas(Gas::bls_pairing_gas_price(n as i64))?;

    let res = if n == 0 {
        false
    } else {
        fetch_stack(engine, n * 2)?;
        let mut g1_x = Vec::with_capacity(n);
        let mut g2_y = Vec::with_capacity(n);
        for i in 0..n {
            g2_y.push(engine.cmd.var(1 + i * 2).as_slice()?.clone().get_next_bytes(BLS_G2_LEN)?);
            g1_x.push(engine.cmd.var(2 + i * 2).as_slice()?.clone().get_next_bytes(BLS_G1_LEN)?);
        }
        let mut g1_x_refs = Vec::<&[u8; BLS_G1_LEN]>::with_capacity(n);
        for point in &g1_x {
            g1_x_refs.push(point.as_slice().try_into()?);
        }
        let mut g2_y_refs = Vec::<&[u8; BLS_G2_LEN]>::with_capacity(n);
        for point in &g2_y {
            g2_y_refs.push(point.as_slice().try_into()?);
        }

        pairing(&g1_x_refs, &g2_y_refs).unwrap_or(false)
    };
    engine.cc.stack.push(boolean!(res));

    Ok(())
}

/// BLS_PUSHR ( - r)
/// Pushes the order of G1 and G2 (approx. `2^255`).
pub(super) fn execute_bls_pushr(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("BLS_PUSHR"))?;
    engine.cc.stack.push(StackItem::Integer(R.clone()));
    Ok(())
}
