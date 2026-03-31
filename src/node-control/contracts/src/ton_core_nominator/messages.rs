/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use ton_block::{BuilderData, Cell, Coins, IBitstring, Serializable};

/// Opcodes for nominator pool contract messages.
///
/// `NEW_STAKE` and `RECOVER_STAKE` share the same opcodes and message format
/// as the single-nominator contract. Reuse `crate::nominator::new_stake` and
/// `crate::nominator::recover_stake` builders for those messages.
pub mod opcodes {
    /// Send new stake to the elector (same as elector/SNP)
    pub const NEW_STAKE: u32 = 0x4e73744b;
    /// Recover stake from the elector (same as elector/SNP)
    pub const RECOVER_STAKE: u32 = 0x47657424;

    // Pool-specific operations (sent as internal messages with query_id)
    /// Accept coins (op = 1)
    pub const ACCEPT_COINS: u32 = 1;
    /// Process pending withdrawal requests (op = 2)
    pub const PROCESS_WITHDRAW_REQUESTS: u32 = 2;
    /// Emergency: process a single withdraw request (op = 3)
    pub const EMERGENCY_WITHDRAW: u32 = 3;
    /// Deposit validator funds (op = 4)
    pub const DEPOSIT_VALIDATOR: u32 = 4;
    /// Withdraw validator funds (op = 5)
    pub const WITHDRAW_VALIDATOR: u32 = 5;
    /// Update current validator set hash (op = 6, anyone can call)
    pub const UPDATE_VALIDATOR_SET: u32 = 6;
    /// Clean up outdated config proposal votings (op = 7)
    pub const CLEANUP_VOTINGS: u32 = 7;
}

/// Build "accept coins" message body (op = 1).
///
/// Credits the attached message value to the pool balance. The contract only checks
/// opcode and `query_id`; TON amount is carried in the message, not in the body.
pub fn accept_coins(query_id: u64) -> anyhow::Result<Cell> {
    let mut builder = BuilderData::new();
    builder.append_u32(opcodes::ACCEPT_COINS)?.append_u64(query_id)?;
    builder.into_cell()
}

/// Build "process withdraw requests" message body.
///
/// Tells the pool to process up to `limit` pending withdrawal requests.
pub fn process_withdraw_requests(query_id: u64, limit: u8) -> anyhow::Result<Cell> {
    let mut builder = BuilderData::new();
    builder
        .append_u32(opcodes::PROCESS_WITHDRAW_REQUESTS)?
        .append_u64(query_id)?
        .append_u8(limit)?;
    builder.into_cell()
}

/// Build "emergency process withdraw request" message body (op = 3).
///
/// Forces processing of a single nominator's withdraw request if the pool balance allows.
/// `request_address` is the nominator account id in basechain: 32 bytes (256 bits), same as
/// in `get_nominator_data` / `list_nominators` (without workchain prefix).
pub fn emergency_withdraw(query_id: u64, request_address: &[u8; 32]) -> anyhow::Result<Cell> {
    let mut builder = BuilderData::new();
    builder
        .append_u32(opcodes::EMERGENCY_WITHDRAW)?
        .append_u64(query_id)?
        .append_raw(request_address, 256)?;
    builder.into_cell()
}

/// Build "update validator set" message body.
///
/// Updates the saved validator set hash in the pool.
/// Can be sent by anyone; the pool checks config param 34 on-chain.
pub fn update_validator_set(query_id: u64) -> anyhow::Result<Cell> {
    let mut builder = BuilderData::new();
    builder.append_u32(opcodes::UPDATE_VALIDATOR_SET)?.append_u64(query_id)?;
    builder.into_cell()
}

/// Build "cleanup votings" message body.
///
/// Removes config proposal votings older than 30 days.
pub fn cleanup_votings(query_id: u64) -> anyhow::Result<Cell> {
    let mut builder = BuilderData::new();
    builder.append_u32(opcodes::CLEANUP_VOTINGS)?.append_u64(query_id)?;
    builder.into_cell()
}

/// Build "deposit validator" message body.
///
/// Validator sends coins to increase their own stake in the pool.
/// Attach the desired amount of TON to the message; 1 TON is deducted as a processing fee.
pub fn deposit_validator(query_id: u64) -> anyhow::Result<Cell> {
    let mut builder = BuilderData::new();
    builder.append_u32(opcodes::DEPOSIT_VALIDATOR)?.append_u64(query_id)?;
    builder.into_cell()
}

/// Build "withdraw validator" message body.
///
/// Validator withdraws funds that do not belong to nominators.
/// Can only be called when pool state == 0 (not participating in validation).
pub fn withdraw_validator(query_id: u64, amount: u64) -> anyhow::Result<Cell> {
    let mut builder = BuilderData::new();
    builder.append_u32(opcodes::WITHDRAW_VALIDATOR)?.append_u64(query_id)?;
    Coins::new(amount).write_to(&mut builder)?;
    builder.into_cell()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ton_block::{Coins, Deserializable, SliceData};

    #[test]
    fn test_accept_coins() {
        let query_id = 42u64;

        let cell = accept_coins(query_id).unwrap();
        let mut slice = SliceData::load_cell(cell).unwrap();

        assert_eq!(slice.get_next_u32().unwrap(), opcodes::ACCEPT_COINS);
        assert_eq!(slice.get_next_u64().unwrap(), query_id);
        assert_eq!(slice.remaining_bits(), 0);
    }

    #[test]
    fn test_process_withdraw_requests() {
        let query_id = 111u64;
        let limit = 40u8;

        let cell = process_withdraw_requests(query_id, limit).unwrap();
        let mut slice = SliceData::load_cell(cell).unwrap();

        assert_eq!(slice.get_next_u32().unwrap(), opcodes::PROCESS_WITHDRAW_REQUESTS);
        assert_eq!(slice.get_next_u64().unwrap(), query_id);
        assert_eq!(slice.get_next_byte().unwrap(), limit);
        assert_eq!(slice.remaining_bits(), 0);
    }

    #[test]
    fn test_emergency_withdraw() {
        let query_id = 99u64;
        let addr = [0xABu8; 32];

        let cell = emergency_withdraw(query_id, &addr).unwrap();
        let mut slice = SliceData::load_cell(cell).unwrap();

        assert_eq!(slice.get_next_u32().unwrap(), opcodes::EMERGENCY_WITHDRAW);
        assert_eq!(slice.get_next_u64().unwrap(), query_id);
        let got = slice.get_next_bits(256).unwrap();
        assert_eq!(got.len(), 32);
        assert_eq!(got, addr.to_vec());
        assert_eq!(slice.remaining_bits(), 0);
    }

    #[test]
    fn test_update_validator_set() {
        let query_id = 222u64;

        let cell = update_validator_set(query_id).unwrap();
        let mut slice = SliceData::load_cell(cell).unwrap();

        assert_eq!(slice.get_next_u32().unwrap(), opcodes::UPDATE_VALIDATOR_SET);
        assert_eq!(slice.get_next_u64().unwrap(), query_id);
        assert_eq!(slice.remaining_bits(), 0);
    }

    #[test]
    fn test_deposit_validator() {
        let query_id = 333u64;

        let cell = deposit_validator(query_id).unwrap();
        let mut slice = SliceData::load_cell(cell).unwrap();

        assert_eq!(slice.get_next_u32().unwrap(), opcodes::DEPOSIT_VALIDATOR);
        assert_eq!(slice.get_next_u64().unwrap(), query_id);
        assert_eq!(slice.remaining_bits(), 0);
    }

    #[test]
    fn test_withdraw_validator() {
        let query_id = 444u64;
        let amount = 5_000_000_000u64;

        let cell = withdraw_validator(query_id, amount).unwrap();
        let mut slice = SliceData::load_cell(cell).unwrap();

        assert_eq!(slice.get_next_u32().unwrap(), opcodes::WITHDRAW_VALIDATOR);
        assert_eq!(slice.get_next_u64().unwrap(), query_id);
        let coins = Coins::construct_from(&mut slice).unwrap();
        assert_eq!(coins.as_u128(), amount as u128);
        assert_eq!(slice.remaining_bits(), 0);
    }

    #[test]
    fn test_cleanup_votings() {
        let query_id = 555u64;

        let cell = cleanup_votings(query_id).unwrap();
        let mut slice = SliceData::load_cell(cell).unwrap();

        assert_eq!(slice.get_next_u32().unwrap(), opcodes::CLEANUP_VOTINGS);
        assert_eq!(slice.get_next_u64().unwrap(), query_id);
        assert_eq!(slice.remaining_bits(), 0);
    }
}
