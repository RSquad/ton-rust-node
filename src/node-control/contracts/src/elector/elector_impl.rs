/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::{ElectionsInfo, ElectorWrapper, FrozenParticipant, Participant, PastElections};
use crate::{ContractProvider, SmartContract};
use anyhow::Context;
use common::tvm_stack_parser::TvmStackParser;
use std::{collections::HashMap, sync::Arc};
use ton_api::ton::tvm::StackEntry;
use ton_block::{Coins, Deserializable, HashmapE, HashmapType, MsgAddressInt};

pub struct ElectorWrapperImpl {
    provider: Arc<dyn ContractProvider>,
    elector_addr: String,
}

impl ElectorWrapperImpl {
    pub fn new(provider: Arc<dyn ContractProvider>) -> Self {
        Self {
            provider,
            elector_addr: "-1:3333333333333333333333333333333333333333333333333333333333333333"
                .to_owned(),
        }
    }
}

#[async_trait::async_trait]
impl SmartContract for ElectorWrapperImpl {
    async fn balance(&self) -> anyhow::Result<u64> {
        Ok(0)
    }
    fn address(&self) -> MsgAddressInt {
        MsgAddressInt::standard(-1, [0x33u8; 32])
    }
}

#[async_trait::async_trait]
impl ElectorWrapper for ElectorWrapperImpl {
    async fn get_active_election_id(&self) -> anyhow::Result<u64> {
        let stack = self
            .provider
            .get_method(self.elector_addr.clone(), "active_election_id", vec![])
            .await?;
        Ok(stack.i64(0).context("stack parser error")? as u64)
    }
    async fn participates_in(&self, pubkey: &[u8]) -> anyhow::Result<Option<Participant>> {
        let n = ton_api::ton::tvm::numberdecimal::NumberDecimal {
            number: "0x".to_owned() + &hex::encode_upper(pubkey),
        };
        let stack_number =
            StackEntry::Tvm_StackEntryNumber(ton_api::ton::tvm::stackentry::StackEntryNumber {
                number: ton_api::ton::tvm::Number::Tvm_NumberDecimal(n),
            });
        let _ = self
            .provider
            .get_method(self.elector_addr.clone(), "participates_in", vec![stack_number])
            .await?;
        Ok(Some(Participant::default()))
    }
    async fn compute_returned_stake(&self, address: &[u8]) -> anyhow::Result<u64> {
        let n = ton_api::ton::tvm::numberdecimal::NumberDecimal {
            number: "0x".to_owned() + &hex::encode_upper(address),
        };
        let stack_number =
            StackEntry::Tvm_StackEntryNumber(ton_api::ton::tvm::stackentry::StackEntryNumber {
                number: ton_api::ton::tvm::Number::Tvm_NumberDecimal(n),
            });

        let stack = self
            .provider
            .get_method(self.elector_addr.clone(), "compute_returned_stake", vec![stack_number])
            .await?;
        Ok(stack.i64(0).context("stack parse error")? as u64)
    }
    async fn elections_info(&self) -> anyhow::Result<ElectionsInfo> {
        let stack = self
            .provider
            .get_method(self.elector_addr.clone(), "participant_list_extended", vec![])
            .await?;
        let parse_stack = || -> anyhow::Result<ElectionsInfo> {
            let election_id = stack.i64(0)? as u64;
            let elect_close = stack.i64(1)? as u64;
            let min_stake = stack.i64(2)? as u64;
            let total_stake = stack.i64(3)? as u64;

            let nodes = stack.list_or_empty(4)?;
            let mut participants = Vec::new();
            for i in 0..nodes.stack.len() {
                let pair = nodes.tuple(i)?;
                let pub_key = pair.number_bytes(0, 32)?;
                let args = pair.tuple(1)?;
                let stake = args.i64(0)? as u64;
                let max_factor = args.i64(1)? as u32;
                let addr = args.number_bytes(2, 32)?;
                let adnl_addr = args.number_bytes(3, 32)?;
                participants.push(Participant {
                    pub_key,
                    adnl_addr: adnl_addr.to_vec(),
                    wallet_addr: addr.to_vec(),
                    max_factor,
                    election_id,
                    stake,
                    stake_message_boc: None,
                });
            }
            let failed = stack.bool(5)?;
            let finished = stack.bool(6)?;
            Ok(ElectionsInfo {
                election_id,
                elect_close,
                min_stake,
                total_stake,
                failed,
                finished,
                participants,
            })
        };
        parse_stack().context("stack parser error")
    }

    async fn past_elections(&self) -> anyhow::Result<Vec<PastElections>> {
        let stack =
            self.provider.get_method(self.elector_addr.clone(), "past_elections", vec![]).await?;
        parse_past_elections_stack(&stack).context("stack parser error")
    }
}

/// Parse result stack of past_elections method
fn parse_past_elections_stack(stack: &TvmStackParser) -> anyhow::Result<Vec<PastElections>> {
    let entries = stack.list_or_empty(0)?;
    let mut elections = Vec::with_capacity(entries.stack.len());
    for i in 0..entries.stack.len() {
        let item = entries.tuple(i)?;
        let election_id = item.i64(0)? as u64;
        let unfreeze_at = item.i64(1)? as u64;
        let stake_held = item.i64(2)? as u64;
        let vset_hash = item.number_bytes(3, 32)?;
        let frozen_dict = item.cell_opt(4)?;
        let hashmap = HashmapE::with_hashmap(256, frozen_dict);
        let mut frozen_map = HashMap::new();
        HashmapType::iterate_slices(&hashmap, |mut key, mut value| {
            let mut wallet_addr = [0u8; 32];
            wallet_addr.copy_from_slice(&value.get_next_bytes(32)?);
            let weight = value.get_next_u64()?;
            let mut amount = Coins::default();
            amount.read_from(&mut value)?;
            let stake = amount.as_u64().unwrap_or(0);
            let banned = value.get_next_bit()?;
            let frozen = FrozenParticipant { wallet_addr, weight, stake, banned };
            let mut pub_key = [0u8; 32];
            pub_key.copy_from_slice(&key.get_next_bytes(32)?);
            frozen_map.insert(pub_key, frozen);
            Ok(true)
        })?;

        let total_stake = item.i64(5)? as u64;
        let bonuses = item.i64(6)? as u64;
        elections.push(PastElections {
            election_id,
            unfreeze_at,
            stake_held,
            vset_hash,
            total_stake,
            bonuses,
            frozen_map,
        });
    }
    Ok(elections)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ton_api::ton::tvm::{
        List, Number, Tuple, cell, list,
        numberdecimal::NumberDecimal,
        stackentry::{StackEntryCell, StackEntryList, StackEntryNumber, StackEntryTuple},
        tuple,
    };
    use ton_block::{BuilderData, Cell, Coins, IBitstring, Serializable, SliceData};

    fn create_number_entry(value: &str) -> StackEntry {
        StackEntry::Tvm_StackEntryNumber(StackEntryNumber {
            number: Number::Tvm_NumberDecimal(NumberDecimal { number: value.to_string() }),
        })
    }

    fn create_list_entry(elements: Vec<StackEntry>) -> StackEntry {
        StackEntry::Tvm_StackEntryList(StackEntryList {
            list: List::Tvm_List(list::List { elements }),
        })
    }

    fn create_tuple_entry(elements: Vec<StackEntry>) -> StackEntry {
        StackEntry::Tvm_StackEntryTuple(StackEntryTuple {
            tuple: Tuple::Tvm_Tuple(tuple::Tuple { elements }),
        })
    }

    fn create_cell_entry(cell: &Cell) -> StackEntry {
        let boc = ton_block::write_boc(cell).unwrap();
        StackEntry::Tvm_StackEntryCell(StackEntryCell { cell: cell::Cell { bytes: boc } })
    }

    #[test]
    fn test_parse_past_elections_from_stack() {
        let election_id = 42;
        let unfreeze_at = 1700000000;
        let stake_held = 3600;
        let vset_hash = "0x0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";
        let total_stake = 1_000_000;
        let bonuses = 12_345;

        let mut hashmap = HashmapE::with_bit_len(256);
        let pubkey = [0xF1; 32];
        let key_b = BuilderData::with_bytes(&pubkey).unwrap();
        let participant = FrozenParticipant {
            wallet_addr: [0x11; 32],
            weight: 100_100_100,
            stake: 123_123_123,
            banned: true,
        };
        let mut value_b = BuilderData::new();
        value_b.append_raw(&participant.wallet_addr, 256).unwrap();
        value_b.append_u64(participant.weight).unwrap();
        Coins::from(participant.stake).write_to(&mut value_b).unwrap();
        value_b.append_bit_bool(participant.banned).unwrap();
        let key_slice = SliceData::load_builder(key_b).unwrap();
        hashmap.set_builder(key_slice, &value_b).unwrap();

        let args = vec![
            create_number_entry(&election_id.to_string()),
            create_number_entry(&unfreeze_at.to_string()),
            create_number_entry(&stake_held.to_string()),
            create_number_entry(vset_hash),
            create_cell_entry(hashmap.data().unwrap()),
            create_number_entry(&total_stake.to_string()),
            create_number_entry(&bonuses.to_string()),
        ];

        let item_tuple = create_tuple_entry(args);

        let stack = TvmStackParser::new(vec![create_list_entry(vec![item_tuple])]);
        let parsed = parse_past_elections_stack(&stack).unwrap();

        assert_eq!(parsed.len(), 1);
        let item = &parsed[0];
        assert_eq!(item.election_id, election_id as u64);
        assert_eq!(item.unfreeze_at, unfreeze_at as u64);
        assert_eq!(item.stake_held, stake_held as u64);
        assert_eq!(hex::encode(&item.vset_hash), vset_hash[2..].to_string());
        assert_eq!(item.total_stake, total_stake as u64);
        assert_eq!(item.bonuses, bonuses as u64);
        let frozen = item.frozen_map.get(&pubkey).expect("missing frozen entry");
        assert_eq!(frozen.wallet_addr, participant.wallet_addr);
        assert_eq!(frozen.weight, participant.weight);
        assert_eq!(frozen.stake, participant.stake);
        assert_eq!(frozen.banned, participant.banned);
    }
}
