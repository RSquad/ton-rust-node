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
use crate::error::TvmError;
use ton_block::{fail, ExceptionCode, Result};

// Gas state
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Gas {
    gas_max: i64,
    gas_limit: i64,
    gas_credit: i64,
    gas_remaining: i64,
    gas_price: i64,
    gas_base: i64,
}

const CELL_LOAD_GAS_PRICE: i64 = 100;
const CELL_RELOAD_GAS_PRICE: i64 = 25;
const CELL_CREATE_GAS_PRICE: i64 = 500;
const EXCEPTION_GAS_PRICE: i64 = 50;
const TUPLE_ENTRY_GAS_PRICE: i64 = 1;
const IMPLICIT_JMPREF_GAS_PRICE: i64 = 10;
const IMPLICIT_RET_GAS_PRICE: i64 = 5;
const FREE_STACK_DEPTH: usize = 32;
const STACK_ENTRY_GAS_PRICE: i64 = 1;
const RUNVM_GAS_PRICE: i64 = 40;
const CHECK_SIGNATURE_THRESHOLD: usize = 10;
const CHECK_SIGNATURE_GAS_PRICE: i64 = 4000;
const CHECK_P256_SIGNATURE_GAS_PRICE: i64 = 3500;
const EC_RECOVER_GAS_PRICE: i64 = 1500;
const SECP256K1_XONLY_PUBKEY_TWEAK_ADD_GAS_PRICE: i64 = 1250;
// const MAX_DATA_DEPTH: usize = 512;

const BLS_VERIFY_GAS_PRICE: i64 = 61000;
const BLS_AGGREGATE_GAS_A: i64 = 4350;
const BLS_AGGREGATE_GAS_B: i64 = 2650;
const BLS_FASTAGGREGATEVERIFY_GAS_A: i64 = 58000;
const BLS_FASTAGGREGATEVERIFY_GAS_B: i64 = 3000;
const BLS_AGGREGATEVERIFY_GAS_A: i64 = 38500;
const BLS_AGGREGATEVERIFY_GAS_B: i64 = 22500;
const BLS_G1_ADD_SUB_GAS_PRICE: i64 = 3900;
const BLS_G1_NEG_GAS_PRICE: i64 = 750;
const BLS_G1_MUL_GAS_PRICE: i64 = 5200;
const BLS_G1_MULTIEXP_GAS_BASE: i64 = 11375;
const BLS_G1_MULTIEXP_GAS_A: i64 = 630;
const BLS_G1_MULTIEXP_GAS_B: i64 = 8820;
const BLS_MAP_TO_G1_GAS_PRICE: i64 = 2350;
const BLS_G1_INGROUP_GAS_PRICE: i64 = 2950;
const BLS_G2_ADD_SUB_GAS_PRICE: i64 = 6100;
const BLS_G2_NEG_GAS_PRICE: i64 = 1550;
const BLS_G2_MUL_GAS_PRICE: i64 = 10550;
const BLS_G2_MULTIEXP_GAS_BASE: i64 = 30388;
const BLS_G2_MULTIEXP_GAS_A: i64 = 1280;
const BLS_G2_MULTIEXP_GAS_B: i64 = 22840;
const BLS_MAP_TO_G2_GAS_PRICE: i64 = 7950;
const BLS_G2_INGROUP_GAS_PRICE: i64 = 4250;
const BLS_PAIRING_GAS_BASE: i64 = 20000;
const BLS_PAIRING_GAS_ELEM: i64 = 11800;

const RISTRETTO_255_FROMHASH_GAS_PRICE: i64 = 600;
const RISTRETTO_255_VALIDATE_GAS_PRICE: i64 = 200;
const RISTRETTO_255_VALIDATE_ADD_PRICE: i64 = 600;
const RISTRETTO_255_VALIDATE_MUL_PRICE: i64 = 2000;
const RISTRETTO_255_VALIDATE_MULBASE_PRICE: i64 = 750;

impl Gas {
    /// Instance for constructors. Empty fields
    pub const fn empty() -> Gas {
        Gas { gas_max: 0, gas_limit: 0, gas_credit: 0, gas_remaining: 0, gas_price: 0, gas_base: 0 }
    }
    pub const fn infinity() -> i64 {
        i64::MAX
    }
    /// Instance for debug and test. Cheat fields
    pub const fn test() -> Gas {
        Gas {
            gas_max: 1000000000,
            gas_limit: 1000000000,
            gas_credit: 0,
            gas_remaining: 1000000000,
            gas_price: 10,
            gas_base: 1000000000,
        }
    }
    /// Instance for release
    pub fn test_with_limit(gas_limit: i64) -> Gas {
        let mut gas = Gas::test();
        gas.new_gas_limit(gas_limit);
        gas
    }
    /// Instance for release
    pub fn test_with_credit(gas_credit: i64) -> Gas {
        Gas::new(0, gas_credit, 1000000000, 10)
    }
    /// Instance for release
    pub const fn new(gas_limit: i64, gas_credit: i64, gas_max: i64, gas_price: i64) -> Gas {
        let gas_remaining = gas_limit + gas_credit;
        Gas { gas_price, gas_limit, gas_max, gas_remaining, gas_credit, gas_base: gas_remaining }
    }
    /// Compute instruction cost
    pub const fn basic_gas_price(
        instruction_length: usize,
        _instruction_references_count: usize,
    ) -> i64 {
        // old formula from spec: (10 + instruction_length + 5 * instruction_references_count) as i64
        (10 + instruction_length) as i64
    }
    pub fn consume_basic(
        &mut self,
        instruction_length: usize,
        _instruction_references_count: usize,
    ) -> i64 {
        // old formula from spec: (10 + instruction_length + 5 * instruction_references_count) as i64
        self.use_gas((10 + instruction_length) as i64)
    }

    /// Compute exception cost
    pub const fn exception_price() -> i64 {
        EXCEPTION_GAS_PRICE
    }
    pub fn consume_exception(&mut self) -> i64 {
        self.use_gas(EXCEPTION_GAS_PRICE)
    }

    /// Compute exception cost
    pub const fn finalize_price() -> i64 {
        CELL_CREATE_GAS_PRICE
    }
    pub fn consume_finalize(&mut self) -> i64 {
        self.use_gas(CELL_CREATE_GAS_PRICE)
    }

    /// Implicit JMP cost
    pub const fn implicit_jmp_price() -> i64 {
        IMPLICIT_JMPREF_GAS_PRICE
    }
    pub fn consume_implicit_jmp(&mut self) -> i64 {
        self.use_gas(IMPLICIT_JMPREF_GAS_PRICE)
    }

    /// Implicit RET cost
    pub const fn implicit_ret_price() -> i64 {
        IMPLICIT_RET_GAS_PRICE
    }
    pub fn consume_implicit_ret(&mut self) -> i64 {
        self.use_gas(IMPLICIT_RET_GAS_PRICE)
    }

    /// Compute exception cost
    pub const fn load_cell_price(first: bool) -> i64 {
        if first {
            CELL_LOAD_GAS_PRICE
        } else {
            CELL_RELOAD_GAS_PRICE
        }
    }

    /// Stack cost
    pub const fn stack_price(stack_depth: usize) -> i64 {
        let depth = if stack_depth > FREE_STACK_DEPTH { stack_depth } else { FREE_STACK_DEPTH };
        STACK_ENTRY_GAS_PRICE * (depth - FREE_STACK_DEPTH) as i64
    }
    pub fn consume_stack(&mut self, stack_depth: usize) -> i64 {
        self.use_gas(STACK_ENTRY_GAS_PRICE * (stack_depth.saturating_sub(FREE_STACK_DEPTH) as i64))
    }

    /// Compute tuple usage cost
    pub const fn tuple_gas_price(tuple_length: usize) -> i64 {
        TUPLE_ENTRY_GAS_PRICE * tuple_length as i64
    }
    pub fn consume_tuple_gas(&mut self, tuple_length: usize) -> i64 {
        self.use_gas(TUPLE_ENTRY_GAS_PRICE * tuple_length as i64)
    }

    pub const fn check_signature_price(count: usize) -> i64 {
        if count > CHECK_SIGNATURE_THRESHOLD {
            CHECK_SIGNATURE_GAS_PRICE
        } else {
            0
        }
    }

    pub const fn check_p256_signature_price() -> i64 {
        CHECK_P256_SIGNATURE_GAS_PRICE
    }

    pub fn bls_verify_gas_price() -> i64 {
        BLS_VERIFY_GAS_PRICE
    }

    pub fn bls_aggregate_gas_price(n: i64) -> i64 {
        n * BLS_AGGREGATE_GAS_A - BLS_AGGREGATE_GAS_B
    }

    pub fn bls_fastaggregateverify_gas_price(n: i64) -> i64 {
        BLS_FASTAGGREGATEVERIFY_GAS_A + n * BLS_FASTAGGREGATEVERIFY_GAS_B
    }

    pub fn bls_aggregateverify_gas_price(n: i64) -> i64 {
        BLS_AGGREGATEVERIFY_GAS_A + n * BLS_AGGREGATEVERIFY_GAS_B
    }

    pub fn bls_g1_add_sub_gas_price() -> i64 {
        BLS_G1_ADD_SUB_GAS_PRICE
    }

    pub fn bls_g1_neg_gas_price() -> i64 {
        BLS_G1_NEG_GAS_PRICE
    }

    pub fn bls_g1_mul_gas_price() -> i64 {
        BLS_G1_MUL_GAS_PRICE
    }

    pub fn bls_g1_multiexp_gas_price(n: i64) -> i64 {
        Self::bls_multiexp_gas_price(
            BLS_G1_MULTIEXP_GAS_BASE,
            n,
            BLS_G1_MULTIEXP_GAS_A,
            BLS_G1_MULTIEXP_GAS_B,
        )
    }

    fn bls_multiexp_gas_price(base: i64, n: i64, a: i64, b: i64) -> i64 {
        // gas = BASE + n * A + n / floor(max(log2(n), 4)) * B
        let mut l = 4;
        while (1 << (l + 1)) <= n {
            l += 1;
        }
        base + n * a + n * b / l
    }

    pub fn bls_map_to_g1_gas_price() -> i64 {
        BLS_MAP_TO_G1_GAS_PRICE
    }

    pub fn bls_g1_ingroup_gas_price() -> i64 {
        BLS_G1_INGROUP_GAS_PRICE
    }

    pub fn bls_g2_add_sub_gas_price() -> i64 {
        BLS_G2_ADD_SUB_GAS_PRICE
    }

    pub fn bls_g2_neg_gas_price() -> i64 {
        BLS_G2_NEG_GAS_PRICE
    }

    pub fn bls_g2_mul_gas_price() -> i64 {
        BLS_G2_MUL_GAS_PRICE
    }

    pub fn bls_g2_multiexp_gas_price(n: i64) -> i64 {
        Self::bls_multiexp_gas_price(
            BLS_G2_MULTIEXP_GAS_BASE,
            n,
            BLS_G2_MULTIEXP_GAS_A,
            BLS_G2_MULTIEXP_GAS_B,
        )
    }

    pub fn bls_map_to_g2_gas_price() -> i64 {
        BLS_MAP_TO_G2_GAS_PRICE
    }

    pub fn bls_g2_ingroup_gas_price() -> i64 {
        BLS_G2_INGROUP_GAS_PRICE
    }

    pub fn bls_pairing_gas_price(n: i64) -> i64 {
        BLS_PAIRING_GAS_BASE + n * BLS_PAIRING_GAS_ELEM
    }

    /// Set input gas to gas limit
    pub fn new_gas_limit(&mut self, gas_limit: i64) {
        self.gas_limit = gas_limit.min(self.gas_max).max(0);
        self.gas_credit = 0;
        self.gas_remaining += self.gas_limit - self.gas_base;
        self.gas_base = self.gas_limit;
    }

    /// Update remaining gas limit
    pub fn use_gas(&mut self, gas: i64) -> i64 {
        self.gas_remaining -= gas;
        self.gas_remaining
    }

    /// Try to consume gas then raise exception out of gas if needed
    pub fn try_use_gas(&mut self, gas: i64) -> Result<Option<i32>> {
        self.gas_remaining -= gas;
        self.check_gas_remaining()
    }

    /// Raise out of gas exception
    pub fn check_gas_remaining(&self) -> Result<Option<i32>> {
        if self.gas_remaining >= 0 {
            Ok(None)
        } else {
            fail!(TvmError::exception(
                ExceptionCode::OutOfGas,
                "out of gas",
                self.gas_base - self.gas_remaining,
                file!(),
                line!()
            ))
        }
    }

    // *** Getters ***
    pub const fn get_gas_price(&self) -> i64 {
        self.gas_price
    }

    pub const fn get_gas_limit(&self) -> i64 {
        self.gas_limit
    }

    pub const fn get_gas_max(&self) -> i64 {
        self.gas_max
    }

    pub const fn get_gas_remaining(&self) -> i64 {
        self.gas_remaining
    }

    pub const fn get_gas_credit(&self) -> i64 {
        self.gas_credit
    }

    pub const fn get_gas_used_full(&self) -> i64 {
        self.gas_base - self.gas_remaining
    }

    pub const fn get_gas_used(&self) -> i64 {
        if self.gas_remaining > 0 {
            self.gas_base - self.gas_remaining
        } else {
            self.gas_base
        }
    }

    pub const fn runvm_gas_price() -> i64 {
        RUNVM_GAS_PRICE
    }

    pub(crate) fn ristretto_255_fromhash_gas_price() -> i64 {
        RISTRETTO_255_FROMHASH_GAS_PRICE
    }

    pub(crate) fn ristretto_255_validate_gas_price() -> i64 {
        RISTRETTO_255_VALIDATE_GAS_PRICE
    }

    pub(crate) fn ristretto_255_add_gas_price() -> i64 {
        RISTRETTO_255_VALIDATE_ADD_PRICE
    }

    pub(crate) fn ristretto_255_mul_gas_price() -> i64 {
        RISTRETTO_255_VALIDATE_MUL_PRICE
    }

    pub(crate) fn ristretto_255_mulbase_gas_price() -> i64 {
        RISTRETTO_255_VALIDATE_MULBASE_PRICE
    }

    pub(crate) fn ec_recover_price() -> i64 {
        EC_RECOVER_GAS_PRICE
    }

    pub(crate) fn secp256k1_xonly_pubkey_tweak_add_price() -> i64 {
        SECP256K1_XONLY_PUBKEY_TWEAK_ADD_GAS_PRICE
    }
}
