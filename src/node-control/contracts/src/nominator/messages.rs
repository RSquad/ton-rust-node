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

// ── TON Core nominator pool (multi-nominator / “nominator-pool” contract) ────────────────
//
// The following module is only for the TON Core pool contract, not the single-nominator (SNP)
// contract. Elector messages `new_stake` / `recover_stake` are identical to SNP — use
// [`new_stake`] and [`recover_stake`] in this file with [`opcodes::NEW_STAKE`] /
// [`opcodes::RECOVER_STAKE`].

/// Message bodies and opcodes **only for the TON Core nominator pool** (multi-nominator pool).
///
/// Internal pool operations use small numeric opcodes (1–7) in the bodies below. Stake to the
/// elector uses the same layout as single-nominator: [`new_stake`] / [`recover_stake`].
pub mod ton_core_pool {
    use ton_block::{BuilderData, Cell, Coins, IBitstring, Serializable};

    /// Opcodes for **TON Core pool** internal messages (not SNP admin messages).
    ///
    /// Stake to elector uses [`super::opcodes::NEW_STAKE`] and [`super::opcodes::RECOVER_STAKE`]
    /// with [`super::new_stake`] / [`super::recover_stake`].
    pub mod opcodes {
        // Pool-specific operations (internal messages with query_id)
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

    /// Fixed fee (nanotons) the TON Core pool contract keeps from each `deposit_validator` inbound
    /// message. The pool credits `message_value - DEPOSIT_VALIDATOR_POOL_FEE_NANOTONS` to
    /// validator stake — send **desired stake + this constant** as the message value.
    pub const DEPOSIT_VALIDATOR_POOL_FEE_NANOTONS: u64 = 1_000_000_000;

    /// Build "accept coins" message body for **TON Core pool** (op = 1).
    ///
    /// Credits the attached message value to the pool balance. The contract only checks
    /// opcode and `query_id`; TON amount is carried in the message, not in the body.
    pub fn accept_coins(query_id: u64) -> anyhow::Result<Cell> {
        let mut builder = BuilderData::new();
        builder.append_u32(opcodes::ACCEPT_COINS)?.append_u64(query_id)?;
        builder.into_cell()
    }

    /// Build "process withdraw requests" body for **TON Core pool**.
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

    /// Build "emergency process withdraw request" body for **TON Core pool** (op = 3).
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

    /// Build "update validator set" body for **TON Core pool**.
    ///
    /// Updates the saved validator set hash in the pool.
    /// Can be sent by anyone; the pool checks config param 34 on-chain.
    pub fn update_validator_set(query_id: u64) -> anyhow::Result<Cell> {
        let mut builder = BuilderData::new();
        builder.append_u32(opcodes::UPDATE_VALIDATOR_SET)?.append_u64(query_id)?;
        builder.into_cell()
    }

    /// Build "cleanup votings" body for **TON Core pool**.
    ///
    /// Removes config proposal votings older than 30 days.
    pub fn cleanup_votings(query_id: u64) -> anyhow::Result<Cell> {
        let mut builder = BuilderData::new();
        builder.append_u32(opcodes::CLEANUP_VOTINGS)?.append_u64(query_id)?;
        builder.into_cell()
    }

    /// Build "deposit validator" body for **TON Core pool**.
    ///
    /// Validator sends coins to increase their own stake in the pool.
    /// Set the message **value** to **desired credited stake + [`DEPOSIT_VALIDATOR_POOL_FEE_NANOTONS`]**
    /// (1 TON); the pool credits `value - fee` to `validator_amount`.
    pub fn deposit_validator(query_id: u64) -> anyhow::Result<Cell> {
        let mut builder = BuilderData::new();
        builder.append_u32(opcodes::DEPOSIT_VALIDATOR)?.append_u64(query_id)?;
        builder.into_cell()
    }

    /// Build "withdraw validator" body for **TON Core pool**.
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
}
