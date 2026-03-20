/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use ton_block::{BuilderData, Cell, Coins, IBitstring, MsgAddressInt, Serializable};

/// Opcodes for single-nominator contract messages
pub mod opcodes {
    /// Withdraw funds to owner's wallet
    pub const WITHDRAW: u32 = 0x1000;
    /// Change the validator address
    pub const CHANGE_VALIDATOR_ADDRESS: u32 = 0x1001;
    /// Send arbitrary message from nominator contract (emergency)
    pub const SEND_RAW_MSG: u32 = 0x7702;
    /// Upgrade nominator contract code (emergency)
    pub const UPGRADE: u32 = 0x9903;
    /// Send new stake to the elector
    pub const NEW_STAKE: u32 = 0x4e73744b;
    /// Recover stake from the elector
    pub const RECOVER_STAKE: u32 = 0x47657424;
}

/// Parameters for new stake message
#[derive(Debug, Clone)]
pub struct NewStakeParams<'a> {
    /// Query ID for the message (must be > 0)
    pub query_id: u64,
    /// Stake amount in nanotons
    pub stake_amount: u64,
    /// Validator public key (256 bits)
    pub validator_pubkey: &'a [u8],
    /// Elections id fetched from elector
    pub stake_at: u32,
    /// Max factor for stake
    pub max_factor: u32,
    /// ADNL address (256 bits)
    pub adnl_addr: &'a [u8],
    /// Signature of the stake params (512 bits)
    pub signature: &'a [u8],
}

fn build_coins(amount: u64) -> anyhow::Result<BuilderData> {
    let coins = Coins::new(amount);
    let mut builder = BuilderData::new();
    coins.write_to(&mut builder)?;
    Ok(builder)
}

/// Build withdraw message body
///
/// Allows owner to withdraw funds from the contract.
/// The contract will leave MIN_TON_FOR_STORAGE (1 TON) in the contract.
pub fn withdraw(query_id: u64, amount: u64) -> anyhow::Result<Cell> {
    let mut builder = BuilderData::new();
    builder
        .append_u32(opcodes::WITHDRAW)?
        .append_u64(query_id)?
        .append_builder(&build_coins(amount)?)?;
    builder.into_cell()
}

/// Build change validator address message body
///
/// Allows owner to change the validator address.
/// Used when validator private key is compromised.
pub fn change_validator(
    query_id: u64,
    new_validator_address: &MsgAddressInt,
) -> anyhow::Result<Cell> {
    let mut builder = BuilderData::new();
    builder.append_u32(opcodes::CHANGE_VALIDATOR_ADDRESS)?.append_u64(query_id)?;
    new_validator_address.write_to(&mut builder)?;
    builder.into_cell()
}

/// Build send raw message body
///
/// Emergency safeguard allowing owner to send arbitrary messages
/// as the nominator contract.
pub fn raw_msg(query_id: u64, mode: u8, msg: Cell) -> anyhow::Result<Cell> {
    let mut builder = BuilderData::new();
    builder
        .append_u32(opcodes::SEND_RAW_MSG)?
        .append_u64(query_id)?
        .append_u8(mode)?
        .checked_append_reference(msg)?;
    builder.into_cell()
}

/// Build upgrade message body
///
/// Emergency safeguard to upgrade nominator contract code.
/// Should never need to use this under normal conditions.
pub fn upgrade(query_id: u64, new_code: Cell) -> anyhow::Result<Cell> {
    let mut builder = BuilderData::new();
    builder
        .append_u32(opcodes::UPGRADE)?
        .append_u64(query_id)?
        .checked_append_reference(new_code)?;
    builder.into_cell()
}

/// Build new stake message body
///
/// Sends stake to the elector for the next validation cycle.
/// Must be sent from validator address.
pub fn new_stake(params: &NewStakeParams) -> anyhow::Result<Cell> {
    // Build the signature cell (stored as reference)
    let signature_cell =
        BuilderData::with_raw(params.signature, params.signature.len() * 8)?.into_cell()?;
    let mut builder = BuilderData::new();
    builder
        .append_u32(opcodes::NEW_STAKE)?
        .append_u64(params.query_id)?
        .append_builder(&build_coins(params.stake_amount)?)?
        .append_raw(params.validator_pubkey, 256)?
        .append_u32(params.stake_at)?
        .append_u32(params.max_factor)?
        .append_raw(params.adnl_addr, 256)?
        .checked_append_reference(signature_cell)?;
    builder.into_cell()
}

/// Build recover stake message body
///
/// Recovers stake from elector of previous validation cycle.
pub fn recover_stake(query_id: u64) -> anyhow::Result<Cell> {
    let mut builder = BuilderData::new();
    builder.append_u32(opcodes::RECOVER_STAKE)?.append_u64(query_id)?;
    builder.into_cell()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ton_block::{Deserializable, SliceData};

    #[test]
    fn test_build_withdraw_message() {
        let query_id = 12345u64;
        let amount = 1_000_000_000u64;

        let cell = withdraw(query_id, amount).unwrap();
        let mut slice = SliceData::load_cell(cell).unwrap();

        let opcode = slice.get_next_u32().unwrap();
        assert_eq!(opcode, opcodes::WITHDRAW);

        let parsed_query_id = slice.get_next_u64().unwrap();
        assert_eq!(parsed_query_id, query_id);

        let coins = Coins::construct_from(&mut slice).unwrap();
        assert_eq!(coins.as_u128(), amount as u128);
    }

    #[test]
    fn test_build_change_validator_message() {
        let query_id = 67890u64;
        let new_validator = MsgAddressInt::with_standart(None, -1, [0xABu8; 32].into()).unwrap();

        let cell = change_validator(query_id, &new_validator).unwrap();
        let mut slice = SliceData::load_cell(cell).unwrap();

        let opcode = slice.get_next_u32().unwrap();
        assert_eq!(opcode, opcodes::CHANGE_VALIDATOR_ADDRESS);

        let parsed_query_id = slice.get_next_u64().unwrap();
        assert_eq!(parsed_query_id, query_id);

        let parsed_addr = MsgAddressInt::construct_from(&mut slice).unwrap();
        assert_eq!(parsed_addr, new_validator);
    }

    #[test]
    fn test_build_send_raw_msg() {
        let query_id = 11111u64;
        let mode = 64u8;

        let mut msg_builder = BuilderData::new();
        msg_builder.append_u32(0xDEADBEEF).unwrap();
        let msg = msg_builder.into_cell().unwrap();

        let cell = raw_msg(query_id, mode, msg.clone()).unwrap();
        let mut slice = SliceData::load_cell(cell).unwrap();

        let opcode = slice.get_next_u32().unwrap();
        assert_eq!(opcode, opcodes::SEND_RAW_MSG);

        let parsed_query_id = slice.get_next_u64().unwrap();
        assert_eq!(parsed_query_id, query_id);

        let parsed_mode = slice.get_next_byte().unwrap();
        assert_eq!(parsed_mode, mode);

        let ref_cell = slice.checked_drain_reference().unwrap();
        assert_eq!(ref_cell.repr_hash(), msg.repr_hash());
    }

    #[test]
    fn test_build_upgrade_message() {
        let query_id = 22222u64;

        let mut code_builder = BuilderData::new();
        code_builder.append_u32(0xCAFEBABE).unwrap();
        let new_code = code_builder.into_cell().unwrap();

        let cell = upgrade(query_id, new_code.clone()).unwrap();
        let mut slice = SliceData::load_cell(cell).unwrap();

        let opcode = slice.get_next_u32().unwrap();
        assert_eq!(opcode, opcodes::UPGRADE);

        let parsed_query_id = slice.get_next_u64().unwrap();
        assert_eq!(parsed_query_id, query_id);

        let ref_cell = slice.checked_drain_reference().unwrap();
        assert_eq!(ref_cell.repr_hash(), new_code.repr_hash());
    }

    #[test]
    fn test_build_new_stake_message() {
        let params = NewStakeParams {
            query_id: 33333u64,
            stake_amount: 10_000_000_000_000u64,
            validator_pubkey: &[0x11u8; 32],
            stake_at: 1700000000u32,
            max_factor: 65536u32,
            adnl_addr: &[0x22u8; 32],
            signature: &[0x33u8; 64],
        };

        let cell = new_stake(&params).unwrap();
        let mut slice = SliceData::load_cell(cell).unwrap();

        let opcode = slice.get_next_u32().unwrap();
        assert_eq!(opcode, opcodes::NEW_STAKE);

        let parsed_query_id = slice.get_next_u64().unwrap();
        assert_eq!(parsed_query_id, params.query_id);

        let coins = Coins::construct_from(&mut slice).unwrap();
        assert_eq!(coins.as_u128(), params.stake_amount as u128);

        let pubkey = slice.get_next_bits(256).unwrap();
        assert_eq!(pubkey, params.validator_pubkey.to_vec());

        let parsed_stake_at = slice.get_next_u32().unwrap();
        assert_eq!(parsed_stake_at, params.stake_at);

        let parsed_max_factor = slice.get_next_u32().unwrap();
        assert_eq!(parsed_max_factor, params.max_factor);

        let adnl = slice.get_next_bits(256).unwrap();
        assert_eq!(adnl, params.adnl_addr.to_vec());

        let sig_cell = slice.checked_drain_reference().unwrap();
        let mut sig_slice = SliceData::load_cell(sig_cell).unwrap();
        let sig = sig_slice.get_next_bits(512).unwrap();
        assert_eq!(sig, params.signature.to_vec());
    }

    #[test]
    fn test_build_recover_stake_message() {
        let query_id = 44444u64;

        let cell = recover_stake(query_id).unwrap();
        let mut slice = SliceData::load_cell(cell).unwrap();

        let opcode = slice.get_next_u32().unwrap();
        assert_eq!(opcode, opcodes::RECOVER_STAKE);

        let parsed_query_id = slice.get_next_u64().unwrap();
        assert_eq!(parsed_query_id, query_id);

        assert_eq!(slice.remaining_bits(), 0);
    }

    #[test]
    fn test_build_coins_helper() {
        let amount = 5_000_000_000u64;

        let builder = build_coins(amount).unwrap();
        let cell = builder.into_cell().unwrap();
        let mut slice = SliceData::load_cell(cell).unwrap();

        let coins = Coins::construct_from(&mut slice).unwrap();
        assert_eq!(coins.as_u128(), amount as u128);
    }

    #[test]
    fn test_build_coins_zero() {
        let amount = 0u64;

        let builder = build_coins(amount).unwrap();
        let cell = builder.into_cell().unwrap();
        let mut slice = SliceData::load_cell(cell).unwrap();

        let coins = Coins::construct_from(&mut slice).unwrap();
        assert_eq!(coins.as_u128(), 0);
    }
}
