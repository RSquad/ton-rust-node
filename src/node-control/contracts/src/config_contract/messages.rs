/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use ton_block::{BuilderData, Cell, IBitstring};

pub mod opcodes {
    pub const VOTE_FOR_PROPOSAL: u32 = 0x566f7465;
}

const VOTE_TAG: u32 = 0x566f7445;

/// Build vote data for signing by validator
pub fn unsigned_vote(validator_idx: u16, proposal_hash: &[u8; 32]) -> anyhow::Result<BuilderData> {
    let mut builder = BuilderData::new();
    builder.append_u32(VOTE_TAG)?.append_u16(validator_idx)?.append_raw(proposal_hash, 256)?;
    Ok(builder)
}

/// Builds vote message body with signature.
pub fn signed_vote(
    query_id: u64,
    unsigned_body: &BuilderData,
    signature: &[u8],
) -> anyhow::Result<Cell> {
    if signature.len() != 64 {
        anyhow::bail!("signature must be 64 bytes, got {}", signature.len());
    }

    let mut builder = BuilderData::new();
    builder
        .append_u32(opcodes::VOTE_FOR_PROPOSAL)?
        .append_u64(query_id)?
        .append_raw(signature, 512)?
        .append_builder(unsigned_body)?;
    builder.into_cell()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ton_block::SliceData;

    #[test]
    fn test_unsigned_vote() {
        let builder = unsigned_vote(42, &[0xAB; 32]).unwrap();
        let cell = builder.into_cell().unwrap();
        let mut slice = SliceData::load_cell(cell).unwrap();

        assert_eq!(slice.get_next_u32().unwrap(), VOTE_TAG);
        assert_eq!(slice.get_next_u16().unwrap(), 42);
        assert_eq!(slice.get_next_bits(256).unwrap(), vec![0xAB; 32]);
        assert_eq!(slice.remaining_bits(), 0);
    }

    #[test]
    fn test_signed_vote() {
        let signature = [0x11u8; 64];
        let body = unsigned_vote(123, &[0xCD; 32]).unwrap();
        let query_id: u64 = 0x1234567890ABCDEF;
        let cell = signed_vote(query_id, &body, &signature).unwrap();
        let mut slice = SliceData::load_cell(cell).unwrap();

        assert_eq!(slice.get_next_u32().unwrap(), opcodes::VOTE_FOR_PROPOSAL);
        assert_eq!(slice.get_next_u64().unwrap(), query_id);
        assert_eq!(slice.get_next_bits(512).unwrap(), signature.to_vec());

        assert_eq!(slice.get_next_u32().unwrap(), VOTE_TAG);
        assert_eq!(slice.get_next_u16().unwrap(), 123);
        assert_eq!(slice.get_next_bits(256).unwrap(), vec![0xCD; 32]);
        assert_eq!(slice.remaining_bits(), 0);
    }

    #[test]
    fn test_signed_vote_invalid_signature_length() {
        let body = unsigned_vote(1, &[0x00; 32]).unwrap();
        let query_id: u64 = 1;

        let result = signed_vote(query_id, &body, &[0u8; 32]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("signature must be 64 bytes"));

        let result = signed_vote(query_id, &body, &[0u8; 128]);
        assert!(result.is_err());
    }
}
