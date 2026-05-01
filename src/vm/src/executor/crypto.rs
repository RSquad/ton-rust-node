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
        integer::{
            behavior::{OperationBehavior, Quiet},
            math::Round,
            IntegerData,
        },
        StackItem,
    },
};
use std::sync::{Arc, LazyLock};
use ton_block::{
    blake2b_digest, fail, keccak256_digest, keccak512_digest, sha256_digest, sha512_digest,
    Bitstring, Ed25519PublicKey, ExceptionCode, GasConsumer, Status, UInt256,
    ED25519_PUBLIC_KEY_LENGTH, ED25519_SIGNATURE_LENGTH, P256_PUBLIC_KEY_LENGTH,
    P256_SIGNATURE_LENGTH,
};

fn hash_to_uint(bits: impl AsRef<[u8]>) -> IntegerData {
    IntegerData::from_unsigned_bytes_be(bits)
}

/// HASHBU (b – x), computes the representation hash of a Cell finalized from Builder c
/// and returns it as a 256-bit unsigned integer x.
pub(super) fn execute_hashbu(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("HASHBU"))?;
    fetch_stack(engine, 1)?;
    let cell = engine.cmd.var_mut(0).as_builder_mut()?.into_cell()?;
    let hash_int = hash_to_uint(cell.repr_hash());
    engine.cc.stack.push(StackItem::integer(hash_int));
    Ok(())
}

/// HASHCU (c – x), computes the representation hash of a Cell c
/// and returns it as a 256-bit unsigned integer x.
pub(super) fn execute_hashcu(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("HASHCU"))?;
    fetch_stack(engine, 1)?;
    let hash_int = hash_to_uint(engine.cmd.var(0).as_cell()?.repr_hash());
    engine.cc.stack.push(StackItem::integer(hash_int));
    Ok(())
}

/// Computes the hash of a Slice s and returns it as a 256-bit unsigned integer x.
/// The result is the same as if an ordinary cell containing only data
/// and references from s had been created and its hash computed by HASHCU.
pub(super) fn execute_hashsu(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("HASHSU"))?;
    fetch_stack(engine, 1)?;
    let builder = engine.cmd.var(0).as_slice()?.as_builder()?;
    let cell = engine.finalize_cell(builder)?;
    let hash_int = hash_to_uint(cell.repr_hash());
    engine.cc.stack.push(StackItem::integer(hash_int));
    Ok(())
}

// SHA256U ( s – x )
// Computes sha256 of the data bits of Slices.
// If the bit length of s is not divisible by eight, throws a cell underflow exception.
// The hash value is returned as a 256-bit unsigned integer x.
pub(super) fn execute_sha256u(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("SHA256U"))?;
    fetch_stack(engine, 1)?;
    let slice = engine.cmd.var(0).as_slice()?;
    if slice.remaining_bits() % 8 == 0 {
        let hash = UInt256::calc_file_hash(&slice.get_bytestring(0));
        let hash_int = hash_to_uint(hash);
        engine.cc.stack.push(StackItem::integer(hash_int));
        Ok(())
    } else {
        fail!(ExceptionCode::CellUnderflow)
    }
}

fn check_signature(engine: &mut Engine, name: &'static str, hash: bool) -> Status {
    engine.load_instruction(Instruction::new(name))?;
    fetch_stack(engine, 3)?;
    let pub_key =
        engine.cmd.var(0).as_integer()?.as_vec(ED25519_PUBLIC_KEY_LENGTH * 8, false, true)?;
    let signature = engine.cmd.var(1).as_slice()?;
    if hash {
        engine.cmd.var(2).as_integer()?;
    } else {
        engine.cmd.var(2).as_slice()?;
    }
    if signature.remaining_bits() < ED25519_SIGNATURE_LENGTH * 8 {
        fail!(ExceptionCode::CellUnderflow)
    }
    let data = if hash {
        engine.cmd.var(2).as_integer()?.as_u256()?
    } else {
        if engine.cmd.var(2).as_slice()?.remaining_bits() % 8 != 0 {
            fail!(ExceptionCode::CellUnderflow)
        }
        engine.cmd.var(2).as_slice()?.get_bytestring(0).into_vec()
    };
    let Ok(pub_key) = Ed25519PublicKey::from_bytes(pub_key.as_slice().try_into()?) else {
        engine.cc.stack.push(boolean!(false));
        return Ok(());
    };
    let signature = engine.cmd.var(1).as_slice()?.get_bytestring(0);
    if signature.len() < ED25519_SIGNATURE_LENGTH {
        engine.cc.stack.push(boolean!(false));
        return Ok(());
    };
    engine.checked_signatures_count = engine.checked_signatures_count.saturating_add(1);
    engine.try_use_gas(Gas::check_signature_price(engine.checked_signatures_count))?;
    let result = engine.modifiers.chksig_always_succeed
        || pub_key.verify(&data, &signature[..ED25519_SIGNATURE_LENGTH].try_into()?);
    engine.cc.stack.push(boolean!(result));
    Ok(())
}

// CHKSIGNS (d s k – -1 or 0)
// checks whether s is a valid Ed25519-signature of the data portion of Slice d using public key k,
// similarly to CHKSIGNU. If the bit length of Slice d is not divisible by eight,
// throws a cell underflow exception. The verification of Ed25519 signatures is the standard one,
// with sha256 used to reduce d to the 256-bit number that is actually signed.
pub(super) fn execute_chksigns(engine: &mut Engine) -> Status {
    check_signature(engine, "CHKSIGNS", false)
}

/// CHKSIGNU (h s k – -1 or 0)
/// checks the Ed25519-signature s (slice) of a hash h (a 256-bit unsigned integer)
/// using public key k (256-bit unsigned integer).
pub(super) fn execute_chksignu(engine: &mut Engine) -> Status {
    check_signature(engine, "CHKSIGNU", true)
}

fn check_p256_signature(engine: &mut Engine, name: &'static str, hash: bool) -> Status {
    engine.load_instruction(Instruction::new(name))?;
    fetch_stack(engine, 3)?;
    let pub_key = engine.cmd.var(0).as_slice()?;
    let signature = engine.cmd.var(1).as_slice()?;
    let data = if hash {
        engine.cmd.var(2).as_integer()?.as_u256()?
    } else {
        if engine.cmd.var(2).as_slice()?.remaining_bits() % 8 != 0 {
            fail!(
                ExceptionCode::CellUnderflow,
                "Slice does not consist of an integer number of bytes"
            )
        }
        engine.cmd.var(2).as_slice()?.get_bytestring(0).into_vec()
    };
    if signature.remaining_bits() < P256_SIGNATURE_LENGTH * 8 {
        fail!(ExceptionCode::CellUnderflow, "P256 signature must contain at least 512 data bits")
    }
    if pub_key.remaining_bits() < P256_PUBLIC_KEY_LENGTH * 8 {
        fail!(ExceptionCode::CellUnderflow, "P256 public key must contain at least 33 data bytes")
    }
    let signature = signature.clone().get_next_bytes(P256_SIGNATURE_LENGTH)?;
    let signature = signature.as_slice().try_into()?;

    let pub_key = pub_key.clone().get_next_bytes(P256_PUBLIC_KEY_LENGTH)?;
    let pub_key = pub_key.as_slice().try_into()?;

    engine.try_use_gas(Gas::check_p256_signature_price())?;

    let result = engine.modifiers.chksig_always_succeed
        || ton_block::p256_verify_signature(pub_key, &data, signature).is_ok();
    engine.cc.stack.push(boolean!(result));
    Ok(())
}

// P256_CHKSIGNS (d s k – -1 or 0)
/// Checks seck256r1-signature sig of data portion of slice d and public key k.
/// Returns -1 on success, 0 on failure. Public key is a 33-byte slice
/// (encoded according to Sec. 2.3.4 point 2 of SECG SEC 1).
/// Signature sig is a 64-byte slice (two 256-bit unsigned integers r and s).
pub(super) fn execute_p256_chksigns(engine: &mut Engine) -> Status {
    check_p256_signature(engine, "P256_CHKSIGNS", false)
}

/// P256_CHKSIGNU (h s k – -1 or 0)
/// Checks seck256r1-signature sig of a number h (a 256-bit unsigned integer, usually computed
/// as the hash of some data) and public key k. Returns -1 on success, 0 on failure.
/// Public key is a 33-byte slice (encoded according to Sec. 2.3.4 point 2 of SECG SEC 1).
/// Signature sig is a 64-byte slice (two 256-bit unsigned integers r and s).
pub(super) fn execute_p256_chksignu(engine: &mut Engine) -> Status {
    check_p256_signature(engine, "P256_CHKSIGNU", true)
}

fn serialize_uncompresed_public_key(
    engine: &mut Engine,
    public_key_result: ton_block::Result<[u8; 65]>,
) -> Status {
    if let Ok(public_key) = public_key_result {
        engine.cc.stack.push(StackItem::integer(IntegerData::from_u32(public_key[0] as u32)));
        engine
            .cc
            .stack
            .push(StackItem::integer(IntegerData::from_unsigned_bytes_be(&public_key[1..33])));
        engine
            .cc
            .stack
            .push(StackItem::integer(IntegerData::from_unsigned_bytes_be(&public_key[33..])));
        engine.cc.stack.push(boolean!(true));
    } else {
        engine.cc.stack.push(boolean!(false));
    }
    Ok(())
}

/// ECRECOVER (hash v r s - 0 or h x1 x2 -1)
/// Recovers public key from signature, identical to Bitcoin/Ethereum operations.
/// Takes 32-byte hash as uint256 hash; 65-byte signature as uint8 v and uint256 r, s.
/// Returns 0 on failure, public key and -1 on success.
/// 65-byte public key is returned as uint8 h, uint256 x1, x2.
pub(super) fn execute_ec_recover(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("ECRECOVER"))?;
    fetch_stack(engine, 4)?;
    let s = engine.cmd.var(0).as_integer()?;
    let r = engine.cmd.var(1).as_integer()?;
    let v = engine.cmd.var(2).as_integer()?;
    let hash = engine.cmd.var(3).as_integer()?;
    let mut signature = r.as_u256()?;
    signature.extend_from_slice(&s.as_u256()?);
    let recovery_id = v.as_integer_value(0..=255)?;
    let hash = hash.as_u256()?;
    engine.try_use_gas(Gas::ec_recover_price())?;
    let result = ton_block::secp256k1_recover_public_key(
        hash.as_slice().try_into()?,
        signature.as_slice().try_into()?,
        recovery_id,
    );
    serialize_uncompresed_public_key(engine, result)
}

/// SECP256K1_XONLY_PUBKEY_TWEAK_ADD (k t – 0 or h x1 x2 -1)
pub(super) fn execute_secp256k1_xonly_pubkey_tweak_add(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("SECP256K1_XONLY_PUBKEY_TWEAK_ADD"))?;
    fetch_stack(engine, 2)?;
    let tweak = engine.cmd.var(0).as_integer()?;
    let key = engine.cmd.var(1).as_integer()?;
    let tweak = tweak.as_u256()?;
    let key = key.as_u256()?;
    engine.try_use_gas(Gas::secp256k1_xonly_pubkey_tweak_add_price())?;
    let result = ton_block::secp256k1_xonly_pubkey_tweak_add(
        key.as_slice().try_into()?,
        tweak.as_slice().try_into()?,
    );
    serialize_uncompresed_public_key(engine, result)
}

/// RIST255_FROMHASH (h1 h2 – x)
/// Deterministically generates a valid point x from a 512-bit hash (given as two 256-bit integers).
/// returns point as integer
pub(super) fn execute_ristretto_255_from_hash(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("RIST255_FROMHASH"))?;
    fetch_stack(engine, 2)?;
    engine.cmd.var(0).as_integer()?;
    engine.cmd.var(1).as_integer()?;
    engine.try_use_gas(Gas::ristretto_255_fromhash_gas_price())?;
    let h1 = engine.cmd.var(1).as_integer()?;
    let h2 = engine.cmd.var(0).as_integer()?;
    let mut h1 = h1.as_u256()?;
    let h2 = h2.as_u256()?;
    h1.extend_from_slice(&h2);
    let r = ton_block::ristretto_255_from_hash(h1.as_slice().try_into()?);
    let r = IntegerData::from_unsigned_bytes_be(r);
    engine.cc.stack.push(StackItem::integer(r));
    Ok(())
}

/// RIST255_VALIDATE (x – )
/// RIST255_QVALIDATE (x – 0 or -1)
/// Checks that integer x is a valid representation of some curve point. Throws range_chk on error.
pub(super) fn execute_ristretto_255_validate<T: OperationBehavior>(engine: &mut Engine) -> Status {
    let name = if T::quiet() { "RIST255_QVALIDATE" } else { "RIST255_VALIDATE" };
    engine.load_instruction(Instruction::new(name))?;
    fetch_stack(engine, 1)?;
    engine.cmd.var(0).as_integer()?;
    engine.try_use_gas(Gas::ristretto_255_validate_gas_price())?;

    let x = engine.cmd.var(0).as_integer()?;
    if let Ok(x) = x.as_u256() {
        if ton_block::ristretto_255_from_compressed(x.as_slice().try_into()?).is_some() {
            if T::quiet() {
                engine.cc.stack.push(boolean!(true));
            }
            return Ok(());
        }
    }
    if T::quiet() {
        engine.cc.stack.push(boolean!(false));
        Ok(())
    } else {
        fail!(ExceptionCode::RangeCheckError, "x is not a valid encoded element")
    }
}

// 2 ^ 252 + 27742317777372353535851937790883648493
static RISTRETTO_255_L: LazyLock<Arc<IntegerData>> = LazyLock::new(|| {
    Arc::new(
        "7237005577332262213973186563042994240857116359379907606001950938285454250989"
            .parse()
            .unwrap(),
    )
});

/// RIST255_ADD (x y – x+y)
/// RIST255_QADD (x y – 0 or x+y -1)
/// Addition of two points on a curve.
pub(super) fn execute_ristretto_255_add<T: OperationBehavior>(engine: &mut Engine) -> Status {
    let name = if T::quiet() { "RIST255_QADD" } else { "RIST255_ADD" };
    engine.load_instruction(Instruction::new(name))?;
    fetch_stack(engine, 2)?;
    engine.cmd.var(0).as_integer()?;
    engine.cmd.var(1).as_integer()?;
    engine.try_use_gas(Gas::ristretto_255_add_gas_price())?;

    let y = engine.cmd.var(0).as_integer()?;
    let x = engine.cmd.var(1).as_integer()?;
    if let Ok(x) = x.as_u256() {
        if let Ok(y) = y.as_u256() {
            if let Some(r) =
                ton_block::ristretto_255_add(x.as_slice().try_into()?, y.as_slice().try_into()?)
            {
                let r = IntegerData::from_unsigned_bytes_be(r);
                engine.cc.stack.push(StackItem::integer(r));
                if T::quiet() {
                    engine.cc.stack.push(boolean!(true));
                }
                return Ok(());
            }
        }
    }
    if T::quiet() {
        engine.cc.stack.push(boolean!(false));
        Ok(())
    } else {
        fail!(ExceptionCode::RangeCheckError, "x or y is not a valid encoded element")
    }
}

/// RIST255_SUB (x y – x-y)
/// RIST255_QSUB (x y – 0 or x-y -1)
/// Subtraction of two points on a curve.
pub(super) fn execute_ristretto_255_sub<T: OperationBehavior>(engine: &mut Engine) -> Status {
    let name = if T::quiet() { "RIST255_QSUB" } else { "RIST255_SUB" };
    engine.load_instruction(Instruction::new(name))?;
    fetch_stack(engine, 2)?;
    engine.cmd.var(0).as_integer()?;
    engine.cmd.var(1).as_integer()?;
    engine.try_use_gas(Gas::ristretto_255_add_gas_price())?;

    let y = engine.cmd.var(0).as_integer()?;
    let x = engine.cmd.var(1).as_integer()?;
    if let Ok(x) = x.as_u256() {
        if let Ok(y) = y.as_u256() {
            if let Some(r) =
                ton_block::ristretto_255_sub(x.as_slice().try_into()?, y.as_slice().try_into()?)
            {
                let r = IntegerData::from_unsigned_bytes_be(r);
                engine.cc.stack.push(StackItem::integer(r));
                if T::quiet() {
                    engine.cc.stack.push(boolean!(true));
                }
                return Ok(());
            }
        }
    }
    if T::quiet() {
        engine.cc.stack.push(boolean!(false));
        Ok(())
    } else {
        fail!(ExceptionCode::RangeCheckError, "x or y is not a valid encoded element")
    }
}

/// RIST255_MUL (x n – x*n)
/// RIST255_QMUL (x n – 0 or x*n -1)
/// Multiplies point x by a scalar n. Any n is valid, including negative.
pub(super) fn execute_ristretto_255_mul<T: OperationBehavior>(engine: &mut Engine) -> Status {
    let name = if T::quiet() { "RIST255_QMUL" } else { "RIST255_MUL" };
    engine.load_instruction(Instruction::new(name))?;
    fetch_stack(engine, 2)?;
    engine.cmd.var(0).as_integer()?;
    engine.cmd.var(1).as_integer()?;
    engine.try_use_gas(Gas::ristretto_255_mul_gas_price())?;

    let (_y, n) =
        engine.cmd.var(0).as_integer()?.div::<Quiet>(&RISTRETTO_255_L, Round::FloorToZero)?;
    let x = engine.cmd.var(1).as_integer()?;
    if let Ok(x) = x.as_u256() {
        if let Ok(n) = n.as_vec(256, true, false) {
            if let Some(r) =
                ton_block::ristretto_255_mul(x.as_slice().try_into()?, n.as_slice().try_into()?)
            {
                let r = IntegerData::from_unsigned_bytes_be(r);
                engine.cc.stack.push(StackItem::integer(r));
                if T::quiet() {
                    engine.cc.stack.push(boolean!(true));
                }
                return Ok(());
            }
        }
    }
    if T::quiet() {
        engine.cc.stack.push(boolean!(false));
        Ok(())
    } else {
        fail!(ExceptionCode::RangeCheckError, "x or y is not a valid encoded element")
    }
}

/// RIST255_MULBASE (n – g*n)
/// RIST255_QMULBASE (n – 0 or g*n -1)
/// Multiplies the generator point g by a scalar n. Any n is valid, including negative.
pub(super) fn execute_ristretto_255_mulbase<T: OperationBehavior>(engine: &mut Engine) -> Status {
    let name = if T::quiet() { "RIST255_QMULBASE" } else { "RIST255_MULBASE" };
    engine.load_instruction(Instruction::new(name))?;
    fetch_stack(engine, 1)?;
    engine.cmd.var(0).as_integer()?;
    engine.try_use_gas(Gas::ristretto_255_mulbase_gas_price())?;

    let (_, n) =
        engine.cmd.var(0).as_integer()?.div::<Quiet>(&RISTRETTO_255_L, Round::FloorToZero)?;
    if let Ok(n) = n.as_vec(256, true, false) {
        let r = ton_block::ristretto_255_mulbase(n.as_slice().try_into()?);
        let r = IntegerData::from_unsigned_bytes_be(r);
        engine.cc.stack.push(StackItem::integer(r));
        if T::quiet() {
            engine.cc.stack.push(boolean!(true));
        }
        return Ok(());
    }
    if T::quiet() {
        engine.cc.stack.push(boolean!(false));
        Ok(())
    } else {
        fail!(ExceptionCode::RangeCheckError, "x or y is not a valid encoded element")
    }
}

/// RIST255_PUSHL ( – l)
/// Pushes integer l=2^252+27742317777372353535851937790883648493, which is the order of the group.
pub(super) fn execute_ristretto_255_pushl(engine: &mut Engine) -> Status {
    engine.load_instruction(Instruction::new("RIST255_PUSHL"))?;
    engine.cc.stack.push(StackItem::Integer(RISTRETTO_255_L.clone()));
    Ok(())
}

enum Hasher {
    Blake2b,
    Keccak256,
    Keccak512,
    Sha256,
    Sha512,
}

impl Hasher {
    fn hash(&self, data: &[u8]) -> Vec<u8> {
        match self {
            Self::Blake2b => blake2b_digest(data).to_vec(),
            Self::Keccak256 => keccak256_digest(data).to_vec(),
            Self::Keccak512 => keccak512_digest(data).to_vec(),
            Self::Sha256 => sha256_digest(data).to_vec(),
            Self::Sha512 => sha512_digest(data).to_vec(),
        }
    }

    fn gas_ratio(&self) -> usize {
        match self {
            Self::Blake2b => 19,
            Self::Keccak256 => 11,
            Self::Keccak512 => 6,
            Self::Sha256 => 33,
            Self::Sha512 => 16,
        }
    }
}

fn calc_hash_ext(
    engine: &mut Engine,
    hasher: Hasher,
    name: &'static str,
    add: bool,
    rev: bool,
) -> Status {
    engine.load_instruction(Instruction::new(name))?;
    fetch_stack(engine, 1)?;
    let mut max = engine.stack().depth();
    if add {
        max = max.saturating_sub(1);
    }
    let num = engine.cmd.var(0).as_integer_value(0..=max)?;
    fetch_stack(engine, num)?;
    let mut total_bits = 0;
    let mut gas_consumed = 0;
    let mut buffer = Bitstring::default();
    for i in 0..num {
        let index = if rev { i + 1 } else { num - i };
        if let Ok(slice) = engine.cmd.var(index).as_slice() {
            total_bits += slice.remaining_bits();
            buffer.append_raw(&slice.get_bytestring(0), slice.remaining_bits())?;
        } else if let Ok(builder) = engine.cmd.var(index).as_builder() {
            total_bits += builder.length_in_bits();
            buffer.append_raw(builder.data(), builder.length_in_bits())?;
        } else {
            fail!(ExceptionCode::TypeCheckError, "item is neither a slice nor a builder")
        }
        let gas_total = i + 1 + total_bits / 8 / hasher.gas_ratio();
        engine.try_use_gas((gas_total - gas_consumed) as i64)?;
        gas_consumed = gas_total;
    }
    if total_bits % 8 != 0 {
        fail!(ExceptionCode::CellUnderflow, "data does not consist of an integer number of bytes")
    }
    let hash = hasher.hash(buffer.data());
    if add {
        fetch_stack(engine, 1)?;
        let mut result = engine.cmd.last_var_mut()?.as_builder_mut()?;
        result.append_raw(&hash, hash.len() * 8)?;
        engine.cc.stack.push_builder(result);
    } else if hash.len() <= 32 {
        engine.cc.stack.push(StackItem::int(IntegerData::from_unsigned_bytes_be(&hash)));
    } else {
        let mut tuple = Vec::new();
        for i in 0..hash.len() / 32 {
            let start = i * 32;
            let end = hash.len().min(start + 32);
            tuple.push(StackItem::int(IntegerData::from_unsigned_bytes_be(&hash[start..end])));
        }
        engine.cc.stack.push_tuple(tuple);
    };
    Ok(())
}

pub(super) fn execute_hashext_sha256(engine: &mut Engine) -> Status {
    calc_hash_ext(engine, Hasher::Sha256, "HASHEXT_SHA256", false, false)
}

pub(super) fn execute_hashext_sha512(engine: &mut Engine) -> Status {
    calc_hash_ext(engine, Hasher::Sha512, "HASHEXT_SHA512", false, false)
}

pub(super) fn execute_hashext_blake2b(engine: &mut Engine) -> Status {
    calc_hash_ext(engine, Hasher::Blake2b, "HASHEXT_BLAKE2B", false, false)
}

pub(super) fn execute_hashext_keccak256(engine: &mut Engine) -> Status {
    calc_hash_ext(engine, Hasher::Keccak256, "HASHEXT_KECCAK256", false, false)
}

pub(super) fn execute_hashext_keccak512(engine: &mut Engine) -> Status {
    calc_hash_ext(engine, Hasher::Keccak512, "HASHEXT_KECCAK512", false, false)
}

pub(super) fn execute_hashext_sha256_add(engine: &mut Engine) -> Status {
    calc_hash_ext(engine, Hasher::Sha256, "HASHEXTA_SHA256", true, false)
}

pub(super) fn execute_hashext_sha512_add(engine: &mut Engine) -> Status {
    calc_hash_ext(engine, Hasher::Sha512, "HASHEXTA_SHA512", true, false)
}

pub(super) fn execute_hashext_blake2b_add(engine: &mut Engine) -> Status {
    calc_hash_ext(engine, Hasher::Blake2b, "HASHEXTA_BLAKE2B", true, false)
}

pub(super) fn execute_hashext_keccak256_add(engine: &mut Engine) -> Status {
    calc_hash_ext(engine, Hasher::Keccak256, "HASHEXTA_KECCAK256", true, false)
}

pub(super) fn execute_hashext_keccak512_add(engine: &mut Engine) -> Status {
    calc_hash_ext(engine, Hasher::Keccak512, "HASHEXTA_KECCAK512", true, false)
}

pub(super) fn execute_hashext_sha256_rev(engine: &mut Engine) -> Status {
    calc_hash_ext(engine, Hasher::Sha256, "HASHEXTR_SHA256", false, true)
}

pub(super) fn execute_hashext_sha512_rev(engine: &mut Engine) -> Status {
    calc_hash_ext(engine, Hasher::Sha512, "HASHEXTR_SHA512", false, true)
}

pub(super) fn execute_hashext_blake2b_rev(engine: &mut Engine) -> Status {
    calc_hash_ext(engine, Hasher::Blake2b, "HASHEXTR_BLAKE2B", false, true)
}

pub(super) fn execute_hashext_keccak256_rev(engine: &mut Engine) -> Status {
    calc_hash_ext(engine, Hasher::Keccak256, "HASHEXTR_KECCAK256", false, true)
}

pub(super) fn execute_hashext_keccak512_rev(engine: &mut Engine) -> Status {
    calc_hash_ext(engine, Hasher::Keccak512, "HASHEXTR_KECCAK512", false, true)
}

pub(super) fn execute_hashext_sha256_add_rev(engine: &mut Engine) -> Status {
    calc_hash_ext(engine, Hasher::Sha256, "HASHEXTAR_SHA256", true, true)
}

pub(super) fn execute_hashext_sha512_add_rev(engine: &mut Engine) -> Status {
    calc_hash_ext(engine, Hasher::Sha512, "HASHEXTAR_SHA512", true, true)
}

pub(super) fn execute_hashext_blake2b_add_rev(engine: &mut Engine) -> Status {
    calc_hash_ext(engine, Hasher::Blake2b, "HASHEXTAR_BLAKE2B", true, true)
}

pub(super) fn execute_hashext_keccak256_add_rev(engine: &mut Engine) -> Status {
    calc_hash_ext(engine, Hasher::Keccak256, "HASHEXTAR_KECCAK256", true, true)
}

pub(super) fn execute_hashext_keccak512_add_rev(engine: &mut Engine) -> Status {
    calc_hash_ext(engine, Hasher::Keccak512, "HASHEXTAR_KECCAK512", true, true)
}
