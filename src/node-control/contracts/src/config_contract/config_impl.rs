/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::{ConfigContractWrapper, ConfigProposal, ProposalHash, ProposedParam};
use crate::{
    ContractProvider, SmartContract,
    stack_utils::{bytes_to_stack_entry, i64_to_stack_entry},
};
use anyhow::Context;
use std::sync::Arc;
use ton_block::MsgAddressInt;

/// Implementation of the configuration contract wrapper
///
/// See: https://github.com/ton-blockchain/ton/blob/master/crypto/smartcont/config-code.fc
pub struct ConfigContractImpl {
    provider: Arc<dyn ContractProvider>,
    config_addr: String,
}

impl ConfigContractImpl {
    pub fn new(provider: Arc<dyn ContractProvider>) -> Self {
        Self {
            provider,
            config_addr: "-1:5555555555555555555555555555555555555555555555555555555555555555"
                .to_owned(),
        }
    }

    #[cfg(test)]
    pub fn with_address(provider: Arc<dyn ContractProvider>, config_addr: MsgAddressInt) -> Self {
        Self { provider, config_addr: config_addr.to_string() }
    }
}

#[async_trait::async_trait]
impl SmartContract for ConfigContractImpl {
    async fn balance(&self) -> anyhow::Result<u64> {
        self.provider.balance(&self.address().await).await
    }

    async fn address(&self) -> MsgAddressInt {
        MsgAddressInt::standard(-1, [0x55u8; 32])
    }
}

#[async_trait::async_trait]
impl ConfigContractWrapper for ConfigContractImpl {
    async fn seqno(&self) -> anyhow::Result<u32> {
        let stack = self.provider.get_method(self.config_addr.clone(), "seqno", vec![]).await?;
        Ok(stack.i64(0)? as u32)
    }

    async fn get_proposal(&self, phash: ProposalHash) -> anyhow::Result<Option<ConfigProposal>> {
        let stack_entry = bytes_to_stack_entry(&phash);
        let stack = self
            .provider
            .get_method(self.config_addr.clone(), "get_proposal", vec![stack_entry])
            .await?;

        if stack.stack.is_empty() {
            return Ok(None);
        }

        // The result is a tuple: [expires, critical?, [param_id, param_val, param_hash],
        // vset_id, voters_list, weight_remaining, rounds_remaining, losses, wins]
        let parse_proposal = || -> anyhow::Result<ConfigProposal> {
            let tuple = stack.tuple(0)?;

            let expires = tuple.i64(0)? as u32;
            let is_critical = tuple.bool(1)?;

            // Parse param tuple [param_id, param_val, param_hash]
            let param_tuple = tuple.tuple(2)?;
            let id = param_tuple.i64(0)? as i32;
            let cell = param_tuple.cell(1).ok();
            let hash = param_tuple.number_bytes(2, 32).ok().map(|h| {
                let mut hash = [0u8; 32];
                hash.copy_from_slice(&h);
                hash
            });

            let mut vset_id = [0u8; 32];
            vset_id.copy_from_slice(&tuple.number_bytes(3, 32)?);

            // Parse voters list
            let voters_list = tuple.list(4)?;
            let mut voters = Vec::new();
            for i in 0..voters_list.stack.len() {
                let voter_idx =
                    u16::try_from(voters_list.i64(i)?).context("voter index out of u16 range")?;
                voters.push(voter_idx);
            }

            let weight_remaining = tuple.i64(5)?;
            let rounds_remaining = tuple.i64(6)? as u8;
            let losses = tuple.i64(7)? as u8;
            let wins = tuple.i64(8)? as u8;

            Ok(ConfigProposal {
                hash: phash,
                expires,
                is_critical,
                param: ProposedParam { id, cell, hash },
                vset_id,
                voters,
                weight_remaining,
                rounds_remaining,
                losses,
                wins,
            })
        };

        parse_proposal().map(Some).context("parse proposal")
    }

    async fn list_proposals(&self) -> anyhow::Result<Vec<ConfigProposal>> {
        let stack =
            self.provider.get_method(self.config_addr.clone(), "list_proposals", vec![]).await?;

        let parse_proposals = || -> anyhow::Result<Vec<ConfigProposal>> {
            let list = stack.list(0)?;
            let mut proposals = Vec::new();

            // Each element in the list is a tuple: [phash, [fields...]]
            for i in 0..list.stack.len() {
                let pair = list.tuple(i)?;
                // the first element is the proposal hash
                let mut phash = [0u8; 32];
                phash.copy_from_slice(&pair.number_bytes(0, 32)?);
                // the second element is the proposal fields
                let fields = pair.tuple(1)?;
                let expires = fields.i64(0)? as u32;
                let is_critical = fields.bool(1)?;
                // Parse param tuple [param_id, param_val, param_hash]
                let param_tuple = fields.tuple(2)?;
                let param = ProposedParam {
                    id: param_tuple.i64(0)? as i32,
                    cell: param_tuple.cell(1).ok(),
                    hash: param_tuple.number_bytes(2, 32).ok().map(|h| {
                        let mut hash = [0u8; 32];
                        hash.copy_from_slice(&h);
                        hash
                    }),
                };
                // parse vset hash
                let mut vset_id = [0u8; 32];
                vset_id.copy_from_slice(&fields.number_bytes(3, 32)?);

                // Parse voters: list of validator indexes
                let voters_list = fields.list(4)?;
                let mut voters = Vec::new();
                for j in 0..voters_list.stack.len() {
                    let voter_idx = u16::try_from(voters_list.i64(j)?)
                        .context("voter index out of u16 range")?;
                    voters.push(voter_idx);
                }

                let weight_remaining = fields.i64(5)?;
                let rounds_remaining = fields.i64(6)? as u8;
                let losses = fields.i64(7)? as u8;
                let wins = fields.i64(8)? as u8;

                proposals.push(ConfigProposal {
                    hash: phash,
                    expires,
                    is_critical,
                    param,
                    vset_id,
                    voters,
                    weight_remaining,
                    rounds_remaining,
                    losses,
                    wins,
                });
            }

            Ok(proposals)
        };

        parse_proposals().context("parse proposals list")
    }

    async fn proposal_storage_price(
        &self,
        critical: bool,
        seconds: u32,
        bits: u32,
        refs: u32,
    ) -> anyhow::Result<i64> {
        let stack = self
            .provider
            .get_method(
                self.config_addr.clone(),
                "proposal_storage_price",
                vec![
                    i64_to_stack_entry(if critical { -1 } else { 0 }),
                    i64_to_stack_entry(seconds as i64),
                    i64_to_stack_entry(bits as i64),
                    i64_to_stack_entry(refs as i64),
                ],
            )
            .await?;

        Ok(stack.i64(0).context("parse storage price")?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::tvm_stack_parser::TvmStackParser;
    use std::collections::HashMap;
    use ton_api::ton::tvm::{
        List, Number, StackEntry, Tuple, cell, list,
        numberdecimal::NumberDecimal,
        stackentry::{StackEntryCell, StackEntryList, StackEntryNumber, StackEntryTuple},
        tuple,
    };
    use ton_block::{BuilderData, Cell, IBitstring, write_boc};

    // ===== Stack entry helper functions =====

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
        let boc = write_boc(cell).unwrap();
        StackEntry::Tvm_StackEntryCell(StackEntryCell { cell: cell::Cell { bytes: boc } })
    }

    fn bytes_to_hex_number(bytes: &[u8; 32]) -> String {
        format!("0x{}", hex::encode_upper(bytes))
    }

    // ===== Mock ContractProvider =====

    type MethodHandler =
        Box<dyn Fn(Vec<StackEntry>) -> anyhow::Result<TvmStackParser> + Send + Sync>;

    struct MockContractProvider {
        handlers: HashMap<String, MethodHandler>,
        balance_value: u64,
    }

    impl MockContractProvider {
        fn new() -> Self {
            Self {
                handlers: HashMap::new(),
                balance_value: 1_000_000_000, // 1 TON default
            }
        }

        fn with_balance(mut self, balance: u64) -> Self {
            self.balance_value = balance;
            self
        }

        fn on_method<F>(mut self, method: &str, handler: F) -> Self
        where
            F: Fn(Vec<StackEntry>) -> anyhow::Result<TvmStackParser> + Send + Sync + 'static,
        {
            self.handlers.insert(method.to_string(), Box::new(handler));
            self
        }
    }

    #[async_trait::async_trait]
    impl ContractProvider for MockContractProvider {
        async fn get_method(
            &self,
            _address: String,
            method: &str,
            stack: Vec<StackEntry>,
        ) -> anyhow::Result<TvmStackParser> {
            if let Some(handler) = self.handlers.get(method) {
                handler(stack)
            } else {
                anyhow::bail!("No handler registered for method: {}", method)
            }
        }

        async fn balance(&self, _address: &MsgAddressInt) -> anyhow::Result<u64> {
            Ok(self.balance_value)
        }
    }

    // ===== Test helpers =====

    fn create_param_tuple(id: i32, cell: Option<&Cell>, hash: Option<&[u8; 32]>) -> StackEntry {
        let cell_entry = cell.map(create_cell_entry).unwrap_or_else(|| {
            // Create empty cell entry for None case
            create_cell_entry(&Cell::default())
        });
        let hash_entry = hash.map_or_else(
            || create_number_entry("0"),
            |h| create_number_entry(&bytes_to_hex_number(h)),
        );
        create_tuple_entry(vec![create_number_entry(&id.to_string()), cell_entry, hash_entry])
    }

    fn create_proposal_tuple(
        expires: u32,
        is_critical: bool,
        param_id: i32,
        param_cell: Option<&Cell>,
        param_hash: Option<&[u8; 32]>,
        vset_id: &[u8; 32],
        voters: &[u16],
        weight_remaining: i64,
        rounds_remaining: u8,
        losses: u8,
        wins: u8,
    ) -> StackEntry {
        let voters_list =
            create_list_entry(voters.iter().map(|v| create_number_entry(&v.to_string())).collect());
        create_tuple_entry(vec![
            create_number_entry(&expires.to_string()),
            create_number_entry(if is_critical { "-1" } else { "0" }),
            create_param_tuple(param_id, param_cell, param_hash),
            create_number_entry(&bytes_to_hex_number(vset_id)),
            voters_list,
            create_number_entry(&weight_remaining.to_string()),
            create_number_entry(&rounds_remaining.to_string()),
            create_number_entry(&losses.to_string()),
            create_number_entry(&wins.to_string()),
        ])
    }

    // ===== Tests for seqno() =====

    #[tokio::test]
    async fn test_seqno_returns_value() {
        let provider = MockContractProvider::new()
            .on_method("seqno", |_| Ok(TvmStackParser::new(vec![create_number_entry("42")])));

        let config = ConfigContractImpl::new(Arc::new(provider));
        let seqno = config.seqno().await.expect("Failed to get seqno");
        assert_eq!(seqno, 42);
    }

    #[tokio::test]
    async fn test_seqno_returns_large_value() {
        let provider = MockContractProvider::new().on_method("seqno", |_| {
            Ok(TvmStackParser::new(vec![create_number_entry("4294967295")])) // u32::MAX
        });

        let config = ConfigContractImpl::new(Arc::new(provider));
        let seqno = config.seqno().await.expect("Failed to get seqno");
        assert_eq!(seqno, u32::MAX);
    }

    #[tokio::test]
    async fn test_seqno_returns_zero() {
        let provider = MockContractProvider::new()
            .on_method("seqno", |_| Ok(TvmStackParser::new(vec![create_number_entry("0")])));

        let config = ConfigContractImpl::new(Arc::new(provider));
        let seqno = config.seqno().await.expect("Failed to get seqno");
        assert_eq!(seqno, 0);
    }

    #[tokio::test]
    async fn test_seqno_hex_format() {
        let provider = MockContractProvider::new()
            .on_method("seqno", |_| Ok(TvmStackParser::new(vec![create_number_entry("0xFF")])));

        let config = ConfigContractImpl::new(Arc::new(provider));
        let seqno = config.seqno().await.expect("Failed to get seqno");
        assert_eq!(seqno, 255);
    }

    // ===== Tests for get_proposal() =====

    #[tokio::test]
    async fn test_get_proposal_returns_none_for_empty_stack() {
        let provider = MockContractProvider::new()
            .on_method("get_proposal", |_| Ok(TvmStackParser::new(vec![])));

        let config = ConfigContractImpl::new(Arc::new(provider));
        let phash = [0x11u8; 32];
        let proposal = config.get_proposal(phash).await.expect("Failed to get proposal");
        assert!(proposal.is_none());
    }

    #[tokio::test]
    async fn test_get_proposal_returns_proposal() {
        let vset_id = [0xABu8; 32];
        let param_hash = [0xCDu8; 32];

        let provider = MockContractProvider::new().on_method("get_proposal", move |_| {
            let tuple = create_proposal_tuple(
                1700000000, // expires
                true,       // is_critical
                15,         // param_id
                None,       // param_cell
                Some(&param_hash),
                &vset_id,
                &[1, 2, 3], // voters
                1000,       // weight_remaining
                5,          // rounds_remaining
                2,          // losses
                3,          // wins
            );
            Ok(TvmStackParser::new(vec![tuple]))
        });

        let config = ConfigContractImpl::new(Arc::new(provider));
        let phash = [0x11u8; 32];
        let proposal = config.get_proposal(phash).await.expect("Failed to get proposal");

        let p = proposal.expect("Expected Some proposal");
        assert_eq!(p.hash, phash);
        assert_eq!(p.expires, 1700000000);
        assert!(p.is_critical);
        assert_eq!(p.param.id, 15);
        assert_eq!(p.param.hash, Some(param_hash));
        assert_eq!(p.vset_id, vset_id);
        assert_eq!(p.voters, vec![1, 2, 3]);
        assert_eq!(p.weight_remaining, 1000);
        assert_eq!(p.rounds_remaining, 5);
        assert_eq!(p.losses, 2);
        assert_eq!(p.wins, 3);
    }

    #[tokio::test]
    async fn test_get_proposal_with_cell() {
        let vset_id = [0x00u8; 32];
        let mut builder = BuilderData::new();
        builder.append_u32(0xDEADBEEF).unwrap();
        let param_cell = builder.into_cell().unwrap();
        let param_cell_clone = param_cell.clone();

        let provider = MockContractProvider::new().on_method("get_proposal", move |_| {
            let tuple = create_tuple_entry(vec![
                create_number_entry("1800000000"), // expires
                create_number_entry("0"),          // is_critical = false
                create_tuple_entry(vec![
                    create_number_entry("20"),            // param_id
                    create_cell_entry(&param_cell_clone), // param_cell
                    create_number_entry("0"),             // param_hash (None)
                ]),
                create_number_entry(&bytes_to_hex_number(&vset_id)),
                create_list_entry(vec![]),  // empty voters
                create_number_entry("500"), // weight_remaining
                create_number_entry("3"),   // rounds_remaining
                create_number_entry("0"),   // losses
                create_number_entry("1"),   // wins
            ]);
            Ok(TvmStackParser::new(vec![tuple]))
        });

        let config = ConfigContractImpl::new(Arc::new(provider));
        let phash = [0x22u8; 32];
        let proposal = config.get_proposal(phash).await.expect("Failed to get proposal");

        let p = proposal.expect("Expected Some proposal");
        assert_eq!(p.expires, 1800000000);
        assert!(!p.is_critical);
        assert_eq!(p.param.id, 20);
        assert!(p.param.cell.is_some());
        assert_eq!(p.param.cell.unwrap().repr_hash(), param_cell.repr_hash());
        assert_eq!(p.voters, Vec::<u16>::new());
        assert_eq!(p.weight_remaining, 500);
        assert_eq!(p.rounds_remaining, 3);
        assert_eq!(p.losses, 0);
        assert_eq!(p.wins, 1);
    }

    // ===== Tests for list_proposals() =====

    #[tokio::test]
    async fn test_list_proposals_empty() {
        let provider = MockContractProvider::new().on_method("list_proposals", |_| {
            Ok(TvmStackParser::new(vec![create_list_entry(vec![])]))
        });

        let config = ConfigContractImpl::new(Arc::new(provider));
        let proposals = config.list_proposals().await.expect("Failed to list proposals");
        assert!(proposals.is_empty());
    }

    #[tokio::test]
    async fn test_list_proposals_single() {
        let phash = [0x44u8; 32];
        let vset_id = [0x55u8; 32];

        let provider = MockContractProvider::new().on_method("list_proposals", move |_| {
            let proposal_fields = create_tuple_entry(vec![
                create_number_entry("2000000000"), // expires
                create_number_entry("-1"),         // is_critical
                create_param_tuple(10, None, None),
                create_number_entry(&bytes_to_hex_number(&vset_id)),
                create_list_entry(vec![create_number_entry("5")]), // voters
                create_number_entry("100"),                        // weight_remaining
                create_number_entry("2"),                          // rounds_remaining
                create_number_entry("1"),                          // losses
                create_number_entry("4"),                          // wins
            ]);
            let proposal_pair = create_tuple_entry(vec![
                create_number_entry(&bytes_to_hex_number(&phash)),
                proposal_fields,
            ]);
            Ok(TvmStackParser::new(vec![create_list_entry(vec![proposal_pair])]))
        });

        let config = ConfigContractImpl::new(Arc::new(provider));
        let proposals = config.list_proposals().await.expect("Failed to list proposals");

        assert_eq!(proposals.len(), 1);
        let p = &proposals[0];
        assert_eq!(p.hash, phash);
        assert_eq!(p.expires, 2000000000);
        assert!(p.is_critical);
        assert_eq!(p.param.id, 10);
        assert_eq!(p.vset_id, vset_id);
        assert_eq!(p.voters, vec![5]);
        assert_eq!(p.weight_remaining, 100);
        assert_eq!(p.rounds_remaining, 2);
        assert_eq!(p.losses, 1);
        assert_eq!(p.wins, 4);
    }

    #[tokio::test]
    async fn test_list_proposals_multiple() {
        let phash1 = [0x11u8; 32];
        let phash2 = [0x22u8; 32];
        let phash3 = [0x33u8; 32];
        let vset_id = [0x00u8; 32];

        let provider = MockContractProvider::new().on_method("list_proposals", move |_| {
            let make_proposal = |phash: &[u8; 32], param_id: i32, wins: u8| {
                let fields = create_tuple_entry(vec![
                    create_number_entry("2100000000"),
                    create_number_entry("0"), // not critical
                    create_param_tuple(param_id, None, None),
                    create_number_entry(&bytes_to_hex_number(&vset_id)),
                    create_list_entry(vec![]),
                    create_number_entry("50"),
                    create_number_entry("1"),
                    create_number_entry("0"),
                    create_number_entry(&wins.to_string()),
                ]);
                create_tuple_entry(vec![create_number_entry(&bytes_to_hex_number(phash)), fields])
            };

            let proposals = vec![
                make_proposal(&phash1, 1, 1),
                make_proposal(&phash2, 2, 2),
                make_proposal(&phash3, 3, 3),
            ];
            Ok(TvmStackParser::new(vec![create_list_entry(proposals)]))
        });

        let config = ConfigContractImpl::new(Arc::new(provider));
        let proposals = config.list_proposals().await.expect("Failed to list proposals");

        assert_eq!(proposals.len(), 3);
        assert_eq!(proposals[0].hash, phash1);
        assert_eq!(proposals[0].param.id, 1);
        assert_eq!(proposals[0].wins, 1);
        assert_eq!(proposals[1].hash, phash2);
        assert_eq!(proposals[1].param.id, 2);
        assert_eq!(proposals[1].wins, 2);
        assert_eq!(proposals[2].hash, phash3);
        assert_eq!(proposals[2].param.id, 3);
        assert_eq!(proposals[2].wins, 3);
    }

    // ===== Tests for proposal_storage_price() =====

    #[tokio::test]
    async fn test_proposal_storage_price_returns_positive() {
        let provider = MockContractProvider::new().on_method("proposal_storage_price", |_| {
            Ok(TvmStackParser::new(vec![create_number_entry("1000000")]))
        });

        let config = ConfigContractImpl::new(Arc::new(provider));
        let price = config
            .proposal_storage_price(false, 86400, 1000, 10)
            .await
            .expect("Failed to get storage price");
        assert_eq!(price, 1_000_000);
    }

    #[tokio::test]
    async fn test_proposal_storage_price_returns_negative_one() {
        let provider = MockContractProvider::new().on_method("proposal_storage_price", |_| {
            Ok(TvmStackParser::new(vec![create_number_entry("-1")]))
        });

        let config = ConfigContractImpl::new(Arc::new(provider));
        let price = config
            .proposal_storage_price(false, 1, 100, 1)
            .await
            .expect("Failed to get storage price");
        assert_eq!(price, -1);
    }

    #[tokio::test]
    async fn test_proposal_storage_price_critical() {
        let provider = MockContractProvider::new().on_method("proposal_storage_price", |args| {
            // Verify critical flag is passed correctly (-1 for true)
            let parser = TvmStackParser::new(args);
            let critical_flag = parser.i64(0).unwrap();
            if critical_flag == -1 {
                Ok(TvmStackParser::new(vec![create_number_entry("2000000")]))
            } else {
                Ok(TvmStackParser::new(vec![create_number_entry("1000000")]))
            }
        });

        let config = ConfigContractImpl::new(Arc::new(provider));
        let price = config
            .proposal_storage_price(true, 86400, 1000, 10)
            .await
            .expect("Failed to get storage price");
        assert_eq!(price, 2_000_000);
    }

    #[tokio::test]
    async fn test_proposal_storage_price_not_critical() {
        let provider = MockContractProvider::new().on_method("proposal_storage_price", |args| {
            // Verify critical flag is passed correctly (0 for false)
            let parser = TvmStackParser::new(args);
            let critical_flag = parser.i64(0).unwrap();
            if critical_flag == 0 {
                Ok(TvmStackParser::new(vec![create_number_entry("500000")]))
            } else {
                Ok(TvmStackParser::new(vec![create_number_entry("1000000")]))
            }
        });

        let config = ConfigContractImpl::new(Arc::new(provider));
        let price = config
            .proposal_storage_price(false, 86400, 1000, 10)
            .await
            .expect("Failed to get storage price");
        assert_eq!(price, 500_000);
    }

    #[tokio::test]
    async fn test_proposal_storage_price_verifies_args() {
        let provider = MockContractProvider::new().on_method("proposal_storage_price", |args| {
            let parser = TvmStackParser::new(args);
            let critical = parser.i64(0).unwrap();
            let seconds = parser.i64(1).unwrap();
            let bits = parser.i64(2).unwrap();
            let refs = parser.i64(3).unwrap();

            // Verify all args are passed correctly
            assert_eq!(critical, 0); // false
            assert_eq!(seconds, 3600);
            assert_eq!(bits, 512);
            assert_eq!(refs, 5);

            Ok(TvmStackParser::new(vec![create_number_entry("12345")]))
        });

        let config = ConfigContractImpl::new(Arc::new(provider));
        let price = config
            .proposal_storage_price(false, 3600, 512, 5)
            .await
            .expect("Failed to get storage price");
        assert_eq!(price, 12345);
    }

    // ===== Tests for SmartContract trait =====

    #[tokio::test]
    async fn test_balance() {
        let provider = MockContractProvider::new().with_balance(5_000_000_000);

        let config = ConfigContractImpl::new(Arc::new(provider));
        let balance = config.balance().await.expect("Failed to get balance");
        assert_eq!(balance, 5_000_000_000);
    }

    #[tokio::test]
    async fn test_address() {
        let provider = MockContractProvider::new();
        let config = ConfigContractImpl::new(Arc::new(provider));
        let addr = config.address().await;
        // Config contract is at -1:5555...5555
        assert_eq!(addr.workchain_id(), -1);
        assert_eq!(addr.address().get_bytestring(0), [0x55u8; 32]);
    }

    #[test]
    fn test_with_address() {
        let provider = MockContractProvider::new();
        let custom_addr = MsgAddressInt::standard(-1, [0xAAu8; 32]);
        let config = ConfigContractImpl::with_address(Arc::new(provider), custom_addr.clone());

        // The address() method returns the hardcoded config address, not the one used for RPC calls
        // But config_addr field is set to the custom address for RPC calls
        assert_eq!(config.config_addr, custom_addr.to_string());
    }

    // ===== Error handling tests =====

    #[tokio::test]
    async fn test_seqno_provider_error() {
        let provider =
            MockContractProvider::new().on_method("seqno", |_| anyhow::bail!("Network error"));

        let config = ConfigContractImpl::new(Arc::new(provider));
        let result = config.seqno().await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Network error"));
    }

    #[tokio::test]
    async fn test_get_proposal_provider_error() {
        let provider =
            MockContractProvider::new().on_method("get_proposal", |_| anyhow::bail!("Error text"));

        let config = ConfigContractImpl::new(Arc::new(provider));
        let result = config.get_proposal([0u8; 32]).await;
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.to_string().contains("Error text"));
    }

    #[tokio::test]
    async fn test_list_proposals_provider_error() {
        let provider = MockContractProvider::new()
            .on_method("list_proposals", |_| anyhow::bail!("Error text"));

        let config = ConfigContractImpl::new(Arc::new(provider));
        let result = config.list_proposals().await;
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.to_string().contains("Error text"));
    }

    #[tokio::test]
    async fn test_proposal_storage_price_provider_error() {
        let provider = MockContractProvider::new()
            .on_method("proposal_storage_price", |_| anyhow::bail!("Error text"));

        let config = ConfigContractImpl::new(Arc::new(provider));
        let result = config.proposal_storage_price(false, 100, 100, 1).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Error text"));
    }
}
