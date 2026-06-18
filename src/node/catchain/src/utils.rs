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
//! Catchain utility functions.
//!
//! Re-exports common utilities from consensus-common and provides catchain-specific utilities.

/// Imports
use crate::{
    ton, BlockHash, BlockPayloadPtr, CatchainFactory, PrivateKey, PublicKey, PublicKeyHash,
    RawBuffer, SessionId,
};
// Re-export macros from consensus-common
pub use consensus_common::serialize_tl_bare_object;
pub use consensus_common::serialize_tl_boxed_object;
// Re-export common utilities from consensus-common
pub use consensus_common::utils::{
    add_compute_percentage_metric, add_compute_relative_metric, add_compute_result_metric,
    bytes_to_string, compute_diff_counter, compute_instance_counter, compute_queue_size_counter,
    compute_result_failure_metric, compute_result_ignore_metric, compute_result_status_metric,
    compute_result_success_metric, deserialize_tl_bare_object, deserialize_tl_boxed_object,
    get_elapsed_time, parse_hex, parse_hex_as_bytes, parse_hex_as_int256, parse_hex_to_array,
    time_to_string, time_to_timestamp_string, Metric, MetricsDumper, MetricsHandle,
};
use secrets_vault::vault_block::get_key_option_factory;
use std::convert::TryInto;
use ton_api::{deserialize_typed, IntoBoxed};
use ton_block::{KeyId, Result, UInt256};

/*
    to string conversions (catchain-specific)
*/

pub fn public_key_hashes_to_string(v: &[PublicKeyHash]) -> String {
    let mut result: String = "[".to_string();
    let mut first = true;

    for key in v {
        if !first {
            result += ", ";
        } else {
            first = false;
        }

        result = format!("{}{}", result, key);
    }

    result + "]"
}

/*
    type conversions (catchain-specific)
*/

pub fn parse_hex_as_block_payload(hex_asm: &str) -> BlockPayloadPtr {
    CatchainFactory::create_block_payload(parse_hex_as_bytes(hex_asm))
}

pub fn parse_hex_as_public_key(hex_asm: &str) -> PublicKey {
    assert!(hex_asm.len() % 2 == 0);
    let mut key_slice = vec![0; hex_asm.len() / 2];
    parse_hex_to_array(hex_asm, &mut key_slice[..]);
    //TODO: errors processing for key creation
    let key = deserialize_typed::<ton_api::ton::PublicKey>(key_slice).unwrap();
    (&key).try_into().unwrap()
}

pub fn parse_hex_as_public_key_raw(hex_asm: &str) -> PublicKey {
    assert!(hex_asm.len() == 64);
    let mut key_slice = [0u8; 32];
    parse_hex_to_array(hex_asm, &mut key_slice[..]);
    //TODO: errors processing for key creation
    get_key_option_factory().from_public_key(&key_slice)
}

pub fn parse_hex_as_public_key_hash(hex_asm: &str) -> PublicKeyHash {
    let mut key_slice = [0; 32];
    parse_hex_to_array(hex_asm, &mut key_slice);
    KeyId::from_data(key_slice)
}

pub fn parse_hex_as_session_id(hex_asm: &str) -> SessionId {
    parse_hex_as_int256(hex_asm)
}

pub fn parse_hex_as_private_key(hex_asm: &str) -> PrivateKey {
    assert!(hex_asm.len() % 2 == 0);
    let mut key_slice = vec![0; hex_asm.len() / 2];
    parse_hex_to_array(hex_asm, &mut key_slice[..]);
    //TODO: errors processing for key creation
    assert!(key_slice.len() == 32);
    get_key_option_factory().from_private_key(key_slice.as_slice().try_into().unwrap()).unwrap()
}

pub fn parse_hex_as_expanded_private_key(hex_asm: &str) -> PrivateKey {
    assert!(hex_asm.len() % 2 == 0);
    let mut key_slice = vec![0; hex_asm.len() / 2];
    parse_hex_to_array(hex_asm, &mut key_slice[..]);
    //TODO: errors processing for key creation
    assert!(key_slice.len() == 64);
    get_key_option_factory().from_expanded_key(key_slice.as_slice().try_into().unwrap()).unwrap()
}

pub fn get_hash(data: &::ton_api::ton::bytes) -> BlockHash {
    UInt256::calc_file_hash(data)
}

pub fn get_hash_from_block_payload(data: &BlockPayloadPtr) -> BlockHash {
    UInt256::calc_file_hash(data.data())
}

pub fn int256_to_public_key_hash(public_key: &UInt256) -> PublicKeyHash {
    KeyId::from_data(*public_key.as_slice())
}

pub fn get_public_key_hash(public_key: &PublicKey) -> PublicKeyHash {
    public_key.id().clone()
}

pub fn public_key_hash_to_int256(v: &PublicKeyHash) -> UInt256 {
    UInt256::with_array(*v.data())
}

pub fn get_overlay_id(first_block: &ton_api::ton::catchain::FirstBlock) -> Result<SessionId> {
    let serialized_first_block = serialize_tl_boxed_object!(first_block);
    let overlay_id = ::ton_api::ton::pub_::publickey::Overlay { name: serialized_first_block };
    let serialized_overlay_id = serialize_tl_boxed_object!(&overlay_id.into_boxed());
    Ok(UInt256::calc_file_hash(&serialized_overlay_id))
}

pub fn get_data_payload_hash(
    data: &ton::BlockData,
    payload: &RawBuffer,
    opts: &crate::Options,
) -> UInt256 {
    let hash = get_hash(payload);

    if !opts.block_hash_covers_data {
        return hash;
    }

    let serialized_data = serialize_tl_boxed_object!(&data.clone().into_boxed());
    let serialized_data_hash = get_hash(&serialized_data);

    assert!(hash.as_slice().len() == 32 && serialized_data_hash.as_slice().len() == 32);

    let mut combined_buffer = [0u8; 64];

    combined_buffer[0..32].copy_from_slice(hash.as_slice());
    combined_buffer[32..64].copy_from_slice(serialized_data_hash.as_slice());
    UInt256::calc_sha256(&combined_buffer)
}

pub fn get_block_id(
    incarnation: &SessionId,
    source_hash: &PublicKeyHash,
    block: &ton::Block,
    payload: &RawBuffer,
    opts: &crate::Options,
) -> ton::BlockId {
    let hash = get_data_payload_hash(&block.data, payload, opts);
    ::ton_api::ton::catchain::block::id::Id {
        incarnation: incarnation.clone(),
        src: public_key_hash_to_int256(source_hash),
        height: block.height,
        data_hash: hash,
    }
    .into_boxed()
}

pub fn get_root_block_id(incarnation: &SessionId) -> ton::BlockId {
    ::ton_api::ton::catchain::block::id::Id {
        incarnation: incarnation.clone(),
        src: incarnation.clone(),
        height: 0,
        data_hash: incarnation.clone(),
    }
    .into_boxed()
}

pub fn get_block_id_hash(id: &ton::BlockId) -> BlockHash {
    let mut serial = Vec::<u8>::new();
    let mut serializer = ton_api::Serializer::new(&mut serial);
    serializer.write_boxed(id).unwrap();
    get_hash(&serial)
}

/*
    serialization utils
*/

pub fn serialize_block_with_payload(
    block: &ton::Block,
    payload: &BlockPayloadPtr,
) -> Result<RawBuffer> {
    let mut raw_data: RawBuffer = RawBuffer::default();
    let mut serializer = ton_api::Serializer::new(&mut raw_data);

    serializer.write_boxed(&block.clone().into_boxed())?;
    raw_data.extend(payload.data().iter());

    Ok(raw_data)
}

pub fn serialize_query_boxed_response<T>(response: Result<T>) -> Result<BlockPayloadPtr>
where
    T: ::ton_api::BoxedSerialize,
{
    match response {
        Ok(response) => {
            let mut ret: RawBuffer = RawBuffer::default();
            let mut serializer = ton_api::Serializer::new(&mut ret);

            serializer.write_boxed(&response).unwrap();

            Ok(CatchainFactory::create_block_payload(ret))
        }
        Err(err) => Err(err),
    }
}
