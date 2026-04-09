/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;
use crate::{
    collator_test_bundle::create_engine_allocated,
    engine_traits::{EngineAlloc, EngineOperations},
    internal_db::{state_gc_resolver::AllowStateGcSmartResolver, InternalDb, InternalDbConfig},
    shard_state::ShardStateStuff,
    shard_states_keeper::PinnedShardStateGuard,
    test_helper::{gen_master_state, gen_shard_state, gen_test_account, GenMasterStateParams},
};
#[cfg(feature = "telemetry")]
use crate::{collator_test_bundle::create_engine_telemetry, engine_traits::EngineTelemetry};
use adnl::common::{Answer, QueryAnswer};
use std::{
    collections::HashMap,
    fs,
    sync::{
        atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicU8, Ordering},
        Arc,
    },
    time::Duration,
};
use storage::block_handle_db::BlockHandle;
use ton_api::{
    serialize_boxed,
    ton::lite_server::{accountid::AccountId as AccountIdTl, BlockHeader, LookupBlockResult},
};
use ton_block::{
    fail, read_single_root_boc, write_boc, AccountDispatchQueue, BlkPrevInfo, Block, BlockExtra,
    BlockIdExt, BlockInfo, ChildCell, ConfigParam0, ConfigParamEnum, ConfigParams,
    CurrencyCollection, DispatchQueue, EnqueuedMsg, ExtBlkRef, GetRepresentationHash,
    IntermediateAddress, InternalMessageHeader, KeyExtBlkRef, KeyId, McBlockExtra, MerkleUpdate,
    Message, MsgAddressInt, MsgEnvelope, OldMcBlocksInfo, OutMsgQueue, OutMsgQueueExtra,
    OutMsgQueueInfo, OutMsgQueueKey, ShardIdent, UInt256, ValueFlow,
};

//static DB_COUNTER: AtomicUsize = AtomicUsize::new(0);
const DB_PATH: &str = "../target/liteserver_test";
const _CONFIG: &str = r#"{
    "p0": "5555555555555555555555555555555555555555555555555555555555555555",
    "p1": "3333333333333333333333333333333333333333333333333333333333333333",
    "p2": "0000000000000000000000000000000000000000000000000000000000000000",
    "p5": {
      "blackhole_addr": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
      "fee_burn_num": 1,
      "fee_burn_denom": 2
    },
    "p7": [
      {
        "currency": 239,
        "value": "666666666666"
      },
      {
        "currency": 4294967279,
        "value": "1000000000000"
      }
    ],
    "p8": {
      "version": 10,
      "capabilities": "494"
    },
    "p9": [
      0,
      1,
      9,
      10,
      12,
      14,
      15,
      16,
      17,
      18,
      20,
      21,
      22,
      23,
      24,
      25,
      28,
      34
    ],
    "p10": [
      0,
      1,
      9,
      10,
      12,
      14,
      15,
      16,
      17,
      32,
      34,
      36,
      4294966295,
      4294966296,
      4294966297
    ],
    "p11": {
      "normal_params": {
        "min_tot_rounds": 2,
        "max_tot_rounds": 3,
        "min_wins": 2,
        "max_losses": 2,
        "min_store_sec": 1000000,
        "max_store_sec": 10000000,
        "bit_price": 1,
        "cell_price": 500
      },
      "critical_params": {
        "min_tot_rounds": 4,
        "max_tot_rounds": 7,
        "min_wins": 4,
        "max_losses": 2,
        "min_store_sec": 5000000,
        "max_store_sec": 20000000,
        "bit_price": 2,
        "cell_price": 1000
      }
    },
    "p12": [
      {
        "workchain_id": 0,
        "enabled_since": 1593639410,
        "monitor_min_split": 2,
        "min_split": 2,
        "max_split": 4,
        "active": true,
        "accept_msgs": true,
        "flags": 0,
        "zerostate_root_hash": "58277424ba5d0b3517b92f53a7cab373baf877bd11ef4b0498dee1a5315c99cd",
        "zerostate_file_hash": "40165518385e945852d5954c95f2a38e0640bb1fb062735afab621293684c753",
        "version": 0,
        "basic": true,
        "vm_version": -1,
        "vm_mode": 0
      }
    ],
    "p13": {
      "boc": "te6ccgEBAQEADQAAFRpRdIdugAEBIB9I"
    },
    "p14": {
      "masterchain_block_fee": "1700000000",
      "basechain_block_fee": "1000000000"
    },
    "p15": {
      "validators_elected_for": 65536,
      "elections_start_before": 32768,
      "elections_end_before": 8192,
      "stake_held_for": 32768
    },
    "p16": {
      "max_validators": 1000,
      "max_main_validators": 100,
      "min_validators": 5
    },
    "p17": {
      "min_stake": "10000000000000",
      "max_stake": "10000000000000000",
      "min_total_stake": "100000000000000",
      "max_stake_factor": 196608
    },
    "p18": [
      {
        "utime_since": 0,
        "bit_price_ps": "1",
        "cell_price_ps": "500",
        "mc_bit_price_ps": "1000",
        "mc_cell_price_ps": "500000"
      }
    ],
    "p20": {
      "flat_gas_limit": "100",
      "flat_gas_price": "1000000",
      "gas_price": "655360000",
      "gas_limit": "1000000",
      "special_gas_limit": "70000000",
      "gas_credit": "10000",
      "block_gas_limit": "2500000",
      "freeze_due_limit": "100000000",
      "delete_due_limit": "1000000000"
    },
    "p21": {
      "flat_gas_limit": "100",
      "flat_gas_price": "40000",
      "gas_price": "26214400",
      "gas_limit": "1000000",
      "special_gas_limit": "1000000",
      "gas_credit": "10000",
      "block_gas_limit": "10000000",
      "freeze_due_limit": "100000000",
      "delete_due_limit": "1000000000"
    },
    "p22": {
      "bytes": {
        "underload": 131072,
        "soft_limit": 524288,
        "hard_limit": 1048576
      },
      "gas": {
        "underload": 2000000,
        "soft_limit": 10000000,
        "hard_limit": 20000000
      },
      "lt_delta": {
        "underload": 1000,
        "soft_limit": 5000,
        "hard_limit": 10000
      }
    },
    "p23": {
      "bytes": {
        "underload": 131072,
        "soft_limit": 524288,
        "hard_limit": 1048576
      },
      "gas": {
        "underload": 2000000,
        "soft_limit": 10000000,
        "hard_limit": 20000000
      },
      "lt_delta": {
        "underload": 1000,
        "soft_limit": 5000,
        "hard_limit": 10000
      }
    },
    "p24": {
      "lump_price": "10000000",
      "bit_price": "655360000",
      "cell_price": "65536000000",
      "ihr_price_factor": 98304,
      "first_frac": 21845,
      "next_frac": 21845
    },
    "p25": {
      "lump_price": "400000",
      "bit_price": "26214400",
      "cell_price": "2621440000",
      "ihr_price_factor": 98304,
      "first_frac": 21845,
      "next_frac": 21845
    },
    "p28": {
      "shuffle_mc_validators": true,
      "isolate_mc_validators": false,
      "mc_catchain_lifetime": 250,
      "shard_catchain_lifetime": 250,
      "shard_validators_lifetime": 1000,
      "shard_validators_num": 7
    },
    "p29": {
      "new_catchain_ids": true,
      "round_candidates": 3,
      "next_candidate_delay_ms": 2000,
      "consensus_timeout_ms": 16000,
      "fast_attempts": 3,
      "attempt_duration": 8,
      "catchain_max_deps": 4,
      "max_block_bytes": 2097152,
      "max_collated_bytes": 2097152,
      "catchain_max_blocks_coeff": 0,
      "proto_version": 0
    },
    "p31": [
      "0000000000000000000000000000000000000000000000000000000000000000",
      "04f64c6afbff3dd10d8ba6707790ac9670d540f37a9448b0337baa6a5a92acac",
      "3333333333333333333333333333333333333333333333333333333333333333",
      "7777777777777777777777777777777777777777777777777777777777777777",
      "8888888888888888888888888888888888888888888888888888888888888888",
      "9999999999999999999999999999999999999999999999999999999999999999"
    ],
    "p32": {
      "utime_since": 1593823221,
      "utime_until": 1593888757,
      "total": 5,
      "main": 5,
      "total_weight": "1152921504606846975",
      "list": [
        {
          "public_key": "1e4497059a4c9854710486ed061699ee482e059ca095f38077ef5536641e89b2",
          "weight": "230584300921369395",
          "adnl_addr": "35c3aac571414e8cc22cd6cb9691837d4489cb44fe41c70f299c323e00dbddff"
        },
        {
          "public_key": "3e04f9101779cdbbaf6c109599dd4038cc634f68de264f062ffd7bd25d94720c",
          "weight": "230584300921369395",
          "adnl_addr": "3d03eb3139ceb434c955837932c28b3efd6add68a2fb2e0a82a435d03062b0d6"
        },
        {
          "public_key": "2850a6f1f3867f48cfff493b180e355a9bdbee61d5d373c7c26fdbcb1589ad50",
          "weight": "230584300921369395",
          "adnl_addr": "9544b802efcdd4d49e368fdd89b879535696d15280a718297c0fa3f9f70fbb6f"
        },
        {
          "public_key": "c4930ad7275d5ddf4107e231bbec337b0dd4610224e43f7744f73d660a09efc3",
          "weight": "230584300921369395",
          "adnl_addr": "bde838908f16cb5875e67b98316c06bd5082f2d234f8ffb2666187d319732db7"
        },
        {
          "public_key": "3259a32efc2f47fc71fcd35cd42dcb84afab9e6d76f972b6fc03a4adf3518036",
          "weight": "230584300921369395",
          "adnl_addr": "e12ae8d5f2e885e07a4674aa462e13df09177253923b7f44bec0c3cc061ce385"
        }
      ]
    },
    "p34": {
      "utime_since": 1593888757,
      "utime_until": 1593954293,
      "total": 5,
      "main": 5,
      "total_weight": "1152921504606846975",
      "list": [
        {
          "public_key": "c4319070e1cac9f777b19377a01ccae4dd803579022073d79bf3eb761c73338f",
          "weight": "329406144173384850",
          "adnl_addr": "4343214d85f1cdea5663151001bce9a5794d5780d53f0e4a7164cb20cf24a0f9"
        },
        {
          "public_key": "2c901a08531cc1f5918f9b3e8b69ce060d7dc7d1e82e776220475ec57e753cd5",
          "weight": "329406144173384850",
          "adnl_addr": "a73c2e823de8c466b87d24440d13e2062ae3edaa00849ff3a36205b7b739e9f3"
        },
        {
          "public_key": "e8cc17da9ada5eca4f1a98bad82f1a599f35b1c73995bf443dae0ce9eb3dc386",
          "weight": "164703072086692425",
          "adnl_addr": "6b25af56150e77ffc8c2b7e5279fa0c54c1e910eac555636989cf7f155d11fa7"
        },
        {
          "public_key": "7f89f047b927adaa26099f2e4028e48a9c5d5c4e8ac68c78aeb924c9b2bf790d",
          "weight": "164703072086692425",
          "adnl_addr": "49656e3861712736dd292436597093c0af16956711b8123777548a840b83ef5a"
        },
        {
          "public_key": "940a2fc654895adac111f8f443dad2cc3f6703dcd6f4dbfa6493f3bbaacd36d2",
          "weight": "164703072086692425",
          "adnl_addr": "7243e081035c836a8fda11ee4b18bad3b15809626c7de5fda0e871eeb885b223"
        }
      ]
    },
    "p43": {
      "max_msg_bits": 2097152,
      "max_msg_cells": 8192,
      "max_library_cells": 1000,
      "max_vm_data_depth": 512,
      "max_ext_msg_size": 65535,
      "max_ext_msg_depth": 512,
      "max_acc_state_cells": 65536,
      "max_acc_state_bits": 67043328,
      "max_acc_public_libraries": 256,
      "defer_out_queue_size_limit": 256,
      "max_msg_extra_currencies": 2,
      "max_acc_fixed_prefix_length": 8
    },
    "p45": {
      "precompiled_contracts_list": {
        "89468f02c78e570802e39979c8516fc38df07ea76a48357e0536f2ba7b3ee37b": 1000
      }
    }
}"#;

#[derive(Clone)]
struct MockShardState {
    pub(crate) accounts: HashMap<UInt256, Vec<u8>>,
    #[allow(dead_code)]
    pub root_hash: UInt256,
}

struct LiteServerTestEngine {
    current_time: u32,
    internal_db: Arc<InternalDb>,
    last_mc_block_id: BlockIdExt,
    mock_blocks: HashMap<BlockIdExt, Vec<u8>>,
    mock_handles: HashMap<BlockIdExt, Arc<BlockHandle>>,
    mock_states: HashMap<BlockIdExt, MockShardState>,
    mock_states_ready: HashMap<BlockIdExt, Arc<ShardStateStuff>>,
    state_delay_ms: AtomicU64,
    zerostate_id: BlockIdExt,
    #[cfg(feature = "telemetry")]
    telemetry: Arc<EngineTelemetry>,
    allocated: Arc<EngineAlloc>,
}

impl LiteServerTestEngine {
    async fn new() -> Self {
        static INDEX: AtomicU16 = AtomicU16::new(0);
        let index = INDEX.fetch_add(1, Ordering::Relaxed);
        let db_path = format!("{}{}", DB_PATH, index);
        fs::remove_dir_all(&db_path).ok();
        let db_config = InternalDbConfig { db_directory: db_path.clone(), ..Default::default() };
        let db = InternalDb::with_update(
            db_config,
            false,
            false,
            false,
            &|| Ok(()),
            None,
            Arc::new(AtomicU8::new(0)),
            None,
            #[cfg(feature = "telemetry")]
            create_engine_telemetry(),
            create_engine_allocated(),
        )
        .await
        .unwrap_or_else(|e| panic!("Can't create db: {e}"));

        let mc_state_id = BlockIdExt {
            shard_id: ShardIdent::with_tagged_prefix(-1, 0x8000000000000000u64).unwrap(),
            seq_no: 50,
            root_hash: UInt256::from([3u8; 32]),
            file_hash: UInt256::from([4u8; 32]),
        };
        let shard_state_id = BlockIdExt {
            shard_id: ShardIdent::with_tagged_prefix(0, 0x8000000000000000u64).unwrap(),
            seq_no: 100,
            root_hash: UInt256::from([5u8; 32]),
            file_hash: UInt256::from([6u8; 32]),
        };
        let mc_state = gen_master_state(
            GenMasterStateParams {
                master_state_id: Some(mc_state_id),
                shard_state_id: Some(shard_state_id),
                ..Default::default()
            },
            #[cfg(feature = "telemetry")]
            None,
            None,
        );
        let (mc_data, mc_state_id) =
            create_minimal_block_boc(mc_state.block_id().clone(), Some(mc_state.root_cell()))
                .unwrap();

        let zerostate_id = BlockIdExt {
            shard_id: ShardIdent::masterchain(),
            seq_no: 0,
            root_hash: UInt256::from([1u8; 32]),
            file_hash: UInt256::from([2u8; 32]),
        };

        let account_id = UInt256::from([0x42u8; 32]);
        let mock_states = HashMap::new();
        let mut accounts_map = HashMap::new();
        accounts_map.insert(account_id.clone(), vec![0x01, 0x02, 0x03, 0x04]);

        let mut ret = Self {
            current_time: 1640995200,
            internal_db: Arc::new(db),
            last_mc_block_id: mc_state_id.clone(),
            zerostate_id,
            mock_blocks: HashMap::new(),
            mock_handles: HashMap::new(),
            mock_states,
            mock_states_ready: HashMap::new(),
            state_delay_ms: AtomicU64::new(0),
            #[cfg(feature = "telemetry")]
            telemetry: create_engine_telemetry(),
            allocated: create_engine_allocated(),
        };
        ret.add_mock_block_with_handle(mc_state_id.clone(), mc_data).await.unwrap();
        ret.add_ready_state(mc_state_id, mc_state);
        ret
    }

    fn add_ready_state(&mut self, id: BlockIdExt, state: Arc<ShardStateStuff>) {
        self.mock_states_ready.insert(id, state);
    }

    async fn add_mock_block_with_handle(&mut self, id: BlockIdExt, data: Vec<u8>) -> Result<()> {
        let cell = read_single_root_boc(&data)?;
        let block = Block::construct_from_cell(cell)?;
        if block.hash()? != id.root_hash || UInt256::calc_file_hash(&data) != id.file_hash {
            fail!("Hashes mismatch in block data");
        }
        println!("Added block {id}");
        let handle = self
            .internal_db
            .create_or_load_block_handle(&id, Some(&block), Some(self.current_time), None)?
            ._to_created()
            .unwrap();
        handle.set_data();
        self.mock_blocks.insert(id.clone(), data);
        self.mock_handles.insert(id, handle);
        Ok(())
    }

    fn create_shard_state_stuff(
        &self,
        block_id: &BlockIdExt,
        mock: &MockShardState,
    ) -> Result<Arc<ShardStateStuff>> {
        let mut accounts: Vec<Account> = Vec::new();
        for (addr, _raw) in &mock.accounts {
            let mut acc = gen_test_account();
            let address = MsgAddressInt::AddrStd(MsgAddrStd {
                anycast: None,
                workchain_id: 0,
                address: addr.clone().into(),
            });
            acc.set_addr(address);
            accounts.push(acc);
        }
        let acc_refs: Vec<&Account> = accounts.iter().collect();

        let (_, state) = gen_shard_state(
            Some(block_id.clone()),
            &acc_refs,
            #[cfg(feature = "telemetry")]
            None,
            None,
            None,
        );
        Ok(state)
    }

    fn set_last_mc_block_id(&mut self, id: BlockIdExt) {
        self.last_mc_block_id = id;
    }

    //fn set_state_delay_ms(&self, delay_ms: u64) {
    //    self.state_delay_ms.store(delay_ms, Ordering::Relaxed);
    //}
}

#[async_trait::async_trait]
impl EngineOperations for LiteServerTestEngine {
    #[cfg(feature = "telemetry")]
    fn engine_telemetry(&self) -> &Arc<EngineTelemetry> {
        &self.telemetry
    }

    fn engine_allocated(&self) -> &Arc<EngineAlloc> {
        &self.allocated
    }

    fn zerostate_id(&self) -> Result<&BlockIdExt> {
        Ok(&self.zerostate_id)
    }

    fn now(&self) -> u32 {
        self.current_time
    }

    fn load_shard_client_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
        Ok(Some(Arc::new(self.last_mc_block_id.clone())))
    }

    fn find_full_block_id(&self, root_hash: &UInt256) -> Result<Option<BlockIdExt>> {
        for (id, _) in &self.mock_blocks {
            if &id.root_hash == root_hash {
                return Ok(Some(id.clone()));
            }
        }
        Ok(None)
    }

    async fn load_state(&self, id: &BlockIdExt) -> Result<Arc<ShardStateStuff>> {
        let delay_ms = self.state_delay_ms.load(Ordering::Relaxed);
        if delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
        if let Some(s) = self.mock_states_ready.get(id) {
            return Ok(s.clone());
        }
        let mock = self.mock_states.get(id).ok_or_else(|| error!("state not found for {id}"))?;
        self.create_shard_state_stuff(id, mock)
    }

    async fn load_and_pin_state(&self, id: &BlockIdExt) -> Result<PinnedShardStateGuard> {
        let state = self.load_state(id).await?;
        PinnedShardStateGuard::new(state, Arc::new(AllowStateGcSmartResolver::new(10)))
    }

    fn load_block_handle(&self, id: &BlockIdExt) -> Result<Option<Arc<BlockHandle>>> {
        println!("Load block {id}");
        Ok(self.mock_handles.get(id).cloned())
    }

    fn load_last_applied_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
        Ok(Some(Arc::new(self.last_mc_block_id.clone())))
    }

    async fn load_block_raw(&self, handle: &BlockHandle) -> Result<Vec<u8>> {
        let id = handle.id();
        self.mock_blocks.get(id).cloned().ok_or_else(|| error!("Block data not found for {id}"))
    }

    async fn load_block(&self, handle: &BlockHandle) -> Result<BlockStuff> {
        let data = self.load_block_raw(handle).await?;
        let stuff = BlockStuff::deserialize_block(handle.id().clone(), Arc::new(data))?;
        Ok(stuff)
    }
}

// fn create_test_account_with_address() -> Account {
//     let account_id = UInt256::from([0x42u8; 32]);

//     let address = MsgAddressInt::AddrStd(MsgAddrStd {
//         anycast: None,
//         workchain_id: 0,
//         address: account_id.clone().into(),
//     });

//     let state_init = StateInit {
//         split_depth: None,
//         special: None,
//         code: Some(BuilderData::new().into_cell().unwrap()),
//         data: Some(BuilderData::new().into_cell().unwrap()),
//         library: StateInitLib::default(),
//     };

//     let account = Account::active(
//         address,
//         CurrencyCollection::with_coins(1000000),
//         1,
//         1640995200,
//         state_init,
//     )
//     .unwrap();
//     account
// }

async fn send_query(
    engine: Arc<LiteServerTestEngine>,
    query: TLObject,
    msg: &str,
) -> Result<TLObject> {
    let runtime = tokio::runtime::Handle::current();
    let subscriber = LiteServerQuerySubscriber::new(runtime, engine, 16, 16, 1)?;
    let query = Query { data: serialize_boxed(&query)? };
    let reply = subscriber
        .try_consume_query(
            query.into_tl_object(),
            &AdnlPeers::with_keys(KeyId::from_data([0u8; 32]), KeyId::from_data([0u8; 32])),
        )
        .await?;
    match reply {
        QueryResult::Consumed(reply) => match reply {
            QueryAnswer::Ready(Some(Answer::Object(reply))) => Ok(reply.object),
            QueryAnswer::Ready(Some(Answer::Raw(_))) => fail!("Raw reply to {msg}"),
            QueryAnswer::Ready(None) => fail!("Empty reply to {msg}"),
            QueryAnswer::Pending(handle) => {
                let reply = handle.await??;
                match reply.answer {
                    Some(Answer::Object(reply)) => Ok(reply.object),
                    Some(Answer::Raw(_)) => fail!("Raw reply to {msg}"),
                    None => fail!("Empty reply to {msg}"),
                }
            }
        },
        QueryResult::Rejected(_) => fail!("Query {msg} rejected"),
        QueryResult::RejectedBundle(_) => fail!("Query {msg} rejected as bundle"),
    }
}

#[tokio::test]
async fn test_lite_server_creation() -> Result<()> {
    let engine = Arc::new(LiteServerTestEngine::new().await);

    let config_str = r#"{
        "address": "127.0.0.1:0",
        "server_key": {
            "type_id": 1209251014,
            "pvt_key": "cJIxGZviebMQWL726DRejqVzRTSXPv/1sO/ab6XOZXk="
        },
        "clients": {
            "list": []
        }
    }"#;

    let config: AdnlServerConfigJson = serde_json::from_str(config_str)?;
    let config = LiteServerConfigJson::from_server_config(config);
    let config = LiteServerConfig::from_json_config(&config)?;
    let runtime = tokio::runtime::Handle::current();
    let lite_server = LiteServer::with_params(config, runtime, engine).await?;

    lite_server.shutdown().await;

    Ok(())
}

#[tokio::test]
async fn test_get_time() -> Result<()> {
    let engine = Arc::new(LiteServerTestEngine::new().await);
    let runtime = tokio::runtime::Handle::current();
    let subscriber = LiteServerQuerySubscriber::new(runtime, engine, 16, 16, 1)?;

    let inner: TLObject = GetTime.into_tl_object();
    let query = Query { data: serialize_boxed(&inner)? };
    let answer = subscriber
        .try_consume_query(
            query.into_tl_object(),
            &AdnlPeers::with_keys(KeyId::from_data([0u8; 32]), KeyId::from_data([0u8; 32])),
        )
        .await
        .unwrap();

    let obj = match answer {
        QueryResult::Consumed(QueryAnswer::Ready(answer)) => match answer {
            Some(Answer::Object(obj)) => obj.object,
            _ => panic!("unexpected answer variant"),
        },
        _ => panic!("query was rejected"),
    };
    use ton_api::ton::lite_server::CurrentTime as LsCurrentTime;
    let result = obj.downcast::<LsCurrentTime>().unwrap();
    assert_eq!(*result.now(), 1640995200);

    Ok(())
}

#[tokio::test]
async fn test_get_version() -> Result<()> {
    let engine = Arc::new(LiteServerTestEngine::new().await);

    let result =
        LiteServerQuerySubscriber::get_version(&(engine.clone() as Arc<dyn EngineOperations>))
            .await?;

    assert_eq!(result.mode, 0);
    assert_eq!(result.version, 0x101);
    assert_eq!(result.capabilities, 0x7);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_liteserver_parallel_requests() -> Result<()> {
    use ton_api::{
        ton::rpc::lite_server::{QueryPrefix, WaitMasterchainSeqno},
        Constructor,
    };
    const DELAY: u64 = 100;
    const CONCURRENCY: usize = 500;

    let engine = Arc::new(LiteServerTestEngine::new().await);
    let runtime = tokio::runtime::Handle::current();
    let subscriber = LiteServerQuerySubscriber::new(
        runtime,
        engine.clone(),
        CONCURRENCY as u64,
        CONCURRENCY as u64,
        1,
    )?;

    // [QueryPrefix TL ID][WaitMasterchainSeqno TL ID][seqno:i32][timeout_ms:i32][query..]
    let inner_query = serialize_boxed(&GetMasterchainInfo.into_tl_object())?;
    let target_seqno = engine.last_mc_block_id.seq_no() as i32;
    let timeout_ms: i32 = 5000;

    let mut data = Vec::new();
    data.extend_from_slice(&QueryPrefix::constructor_const().to_le_bytes());
    data.extend_from_slice(&WaitMasterchainSeqno::constructor_const().to_le_bytes());
    data.extend_from_slice(&target_seqno.to_le_bytes());
    data.extend_from_slice(&timeout_ms.to_le_bytes());
    data.extend_from_slice(&inner_query);

    let mut tasks = Vec::new();
    let start = std::time::Instant::now();
    for _ in 0..CONCURRENCY {
        let query = Query { data: data.clone().into() };
        let reply = subscriber
            .try_consume_query(
                query.into_tl_object(),
                &AdnlPeers::with_keys(KeyId::from_data([0u8; 32]), KeyId::from_data([0u8; 32])),
            )
            .await?;
        match reply {
            QueryResult::Consumed(QueryAnswer::Ready(_)) => {}
            QueryResult::Consumed(QueryAnswer::Pending(handle)) => {
                tasks.push(handle);
            }
            _ => fail!("Unexpected LiteServer reply"),
        }
    }
    for task in tasks {
        task.await??;
    }
    let elapsed = start.elapsed().as_millis();
    println!("Parallel elapsed: {elapsed}ms");

    assert!(elapsed < (DELAY * 2) as u128, "Expected fast completion, got {elapsed}ms");
    Ok(())
}

#[tokio::test]
async fn test_get_account_state_invalid_shard() -> Result<()> {
    let engine = Arc::new(LiteServerTestEngine::new().await);

    let mc_block_id = BlockIdExt {
        shard_id: ShardIdent::masterchain(),
        seq_no: 12345,
        root_hash: UInt256::from([3u8; 32]),
        file_hash: UInt256::from([4u8; 32]),
    };

    let account_id = AccountIdTl { workchain: 0, id: UInt256::from([0x42u8; 32]) };

    let result = LiteServerQuerySubscriber::get_account_state(
        &(engine.clone() as Arc<dyn EngineOperations>),
        mc_block_id,
        account_id,
        true,
        Arc::new(AtomicBool::new(true)),
    )
    .await;

    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_get_masterchain_info() -> Result<()> {
    struct TestEngine {
        state_id: BlockIdExt,
        state: Arc<ShardStateStuff>,
        zerostate_id: BlockIdExt,
    }

    impl TestEngine {
        fn new() -> Self {
            let state = gen_master_state(
                GenMasterStateParams::default(),
                #[cfg(feature = "telemetry")]
                None,
                None,
            );
            let state_id = state.block_id().clone();

            let zerostate_id = BlockIdExt {
                shard_id: ShardIdent::masterchain(),
                seq_no: 0,
                root_hash: UInt256::from([1u8; 32]),
                file_hash: UInt256::from([2u8; 32]),
            };

            Self { state_id, state, zerostate_id }
        }
    }

    #[async_trait::async_trait]
    impl EngineOperations for TestEngine {
        fn zerostate_id(&self) -> Result<&BlockIdExt> {
            Ok(&self.zerostate_id)
        }

        fn load_last_applied_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
            Ok(Some(Arc::new(self.state_id.clone())))
        }

        fn load_shard_client_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
            Ok(None)
        }

        async fn load_state(&self, id: &BlockIdExt) -> Result<Arc<ShardStateStuff>> {
            if *id == self.state_id {
                Ok(self.state.clone())
            } else {
                fail!("Wrong block ID {}", id)
            }
        }

        fn now(&self) -> u32 {
            1640995200
        }
    }

    let engine = Arc::new(TestEngine::new());
    let result = LiteServerQuerySubscriber::get_masterchain_info(
        &(engine.clone() as Arc<dyn EngineOperations>),
    )
    .await?;

    assert_eq!(result.last.shard(), &ShardIdent::masterchain());
    assert_eq!(result.init.workchain, -1);

    Ok(())
}

fn build_minimal_block_from_template(
    tpl: &BlockIdExt,
    shard_state_root: Option<&Cell>,
) -> Result<Block> {
    let prev_root_hash = UInt256::from([0xAB; 32]);
    let prev_file_hash = UInt256::from([0xCD; 32]);

    let mut info = BlockInfo::default();
    info.set_shard(tpl.shard_id.clone());
    info.set_seq_no(tpl.seq_no)?;

    info.set_prev_stuff(
        false,
        &BlkPrevInfo::Block {
            prev: ExtBlkRef {
                end_lt: 1,
                seq_no: tpl.seq_no.saturating_sub(1),
                root_hash: prev_root_hash,
                file_hash: prev_file_hash,
            },
        },
    )?;

    let block = Block::with_params(
        0,
        info,
        ValueFlow::default(),
        if let Some(root) = shard_state_root {
            MerkleUpdate::create(&Cell::default(), root)?
        } else {
            MerkleUpdate::default()
        },
        BlockExtra::default(),
    )?;

    Ok(block)
}

fn create_minimal_block_boc(
    block_id_template: BlockIdExt,
    shard_state_root: Option<&Cell>,
) -> Result<(Vec<u8>, BlockIdExt)> {
    let block = build_minimal_block_from_template(&block_id_template, shard_state_root)?;
    finalize_block_to_boc(block_id_template, &block)
}

fn finalize_block_to_boc(mut id_tpl: BlockIdExt, block: &Block) -> Result<(Vec<u8>, BlockIdExt)> {
    let root = block.serialize()?;
    let root_hash = root.repr_hash();

    let mut bytes = Vec::new();
    BocWriter::with_root(&root)?.write(&mut bytes)?;
    let file_hash = UInt256::calc_file_hash(&bytes);

    id_tpl.root_hash = root_hash;
    id_tpl.file_hash = file_hash;

    Ok((bytes, id_tpl))
}

#[ignore]
#[tokio::test]
async fn test_lookup_block() -> Result<()> {
    let mut engine = LiteServerTestEngine::new().await;
    let tpl = BlockIdExt {
        shard_id: ShardIdent::masterchain(),
        seq_no: 777,
        root_hash: UInt256::default(),
        file_hash: UInt256::default(),
    };
    let (boc, real_id) = create_minimal_block_boc(tpl, None)?;
    engine.add_mock_block_with_handle(real_id.clone(), boc).await?;
    engine.set_last_mc_block_id(real_id.clone());
    let engine = Arc::new(engine);

    // Lookup block
    let block_id = BlockId {
        workchain: real_id.shard().workchain_id(),
        shard: real_id.shard().shard_prefix_with_tag() as i64,
        seqno: real_id.seq_no() as i32,
    };
    let query =
        LookupBlock { mode: 0, id: block_id.clone(), lt: None, utime: None }.into_tl_object();
    let reply =
        send_query(engine.clone(), query, "LookupBlock").await?.downcast::<BlockHeader>().unwrap();
    assert_eq!(reply.id(), &real_id);

    // Lookup block with proof
    let query = LookupBlockWithProof {
        mode: LOOKUP_BY_SEQNO,
        id: block_id.clone(),
        mc_block_id: real_id.clone(),
        lt: None,
        utime: None,
    }
    .into_tl_object();
    let reply = send_query(engine.clone(), query, "LookupBlockWithProof")
        .await?
        .downcast::<LookupBlockResult>()
        .unwrap();
    assert_eq!(reply.id(), &real_id);

    Ok(())
}

#[tokio::test]
async fn test_get_block_success_masterchain() -> Result<()> {
    let mut engine = LiteServerTestEngine::new().await;

    let block_id_template = BlockIdExt {
        shard_id: ShardIdent::masterchain(),
        seq_no: 100,
        root_hash: UInt256::default(),
        file_hash: UInt256::default(),
    };
    let (data, block_id) = create_minimal_block_boc(block_id_template, None)?;
    engine.add_mock_block_with_handle(block_id.clone(), data.clone()).await?;
    let engine = Arc::new(engine);

    let result = LiteServerQuerySubscriber::get_block(
        &(engine.clone() as Arc<dyn EngineOperations>),
        block_id.clone(),
    )
    .await?;

    assert_eq!(result.id, block_id);
    assert_eq!(result.data, data);

    let cell = read_single_root_boc(&result.data)?;
    let block = Block::construct_from_cell(cell)?;
    assert_eq!(block.global_id(), 0);
    assert_eq!(block.read_info()?.seq_no(), block_id.seq_no);
    assert_eq!(block.hash()?, block_id.root_hash);

    Ok(())
}

#[tokio::test]
async fn test_get_block() -> Result<()> {
    let mut engine = LiteServerTestEngine::new().await;

    let block_id_template = BlockIdExt {
        shard_id: ShardIdent::masterchain(),
        seq_no: 100,
        root_hash: UInt256::default(),
        file_hash: UInt256::default(),
    };

    let (data, real_id) = create_minimal_block_boc(block_id_template, None)?;
    engine.add_mock_block_with_handle(real_id.clone(), data.clone()).await?;
    let engine = Arc::new(engine);

    let result = LiteServerQuerySubscriber::get_block(
        &(engine.clone() as Arc<dyn EngineOperations>),
        real_id.clone(),
    )
    .await?;

    assert_eq!(result.id, real_id);
    assert_eq!(result.data, data);

    Ok(())
}

#[ignore]
#[tokio::test]
async fn test_get_block_masterchain_by_seqno_only() -> Result<()> {
    let mut engine = LiteServerTestEngine::new().await;

    let template = BlockIdExt {
        shard_id: ShardIdent::masterchain(),
        seq_no: 100,
        root_hash: UInt256::default(),
        file_hash: UInt256::default(),
    };
    let (data, real_id) = create_minimal_block_boc(template.clone(), None)?;
    engine.add_mock_block_with_handle(real_id.clone(), data.clone()).await?;
    let engine = Arc::new(engine);

    let got = LiteServerQuerySubscriber::get_block(
        &(engine.clone() as Arc<dyn EngineOperations>),
        template,
    )
    .await?;

    assert_eq!(got.id, real_id);
    assert_eq!(got.data, data);
    Ok(())
}

#[tokio::test]
#[ignore]
async fn test_get_state_returns_expected_boc_and_hashes() -> Result<()> {
    struct TestEngine {
        state_id: BlockIdExt,
        state: Arc<ShardStateStuff>,
    }

    impl TestEngine {
        fn new() -> Self {
            let state = gen_master_state(
                GenMasterStateParams::default(),
                #[cfg(feature = "telemetry")]
                None,
                None,
            );
            let state_id = state.block_id().clone();
            Self { state_id, state }
        }
    }

    #[async_trait::async_trait]
    impl EngineOperations for TestEngine {
        async fn load_state(&self, id: &BlockIdExt) -> Result<Arc<ShardStateStuff>> {
            if *id == self.state_id {
                Ok(self.state.clone())
            } else {
                fail!("Wrong block ID {}", id)
            }
        }
        fn now(&self) -> u32 {
            1640995200
        }
    }

    let engine = Arc::new(TestEngine::new());

    let got = LiteServerQuerySubscriber::get_state(
        &(engine.clone() as Arc<dyn EngineOperations>),
        engine.state_id.clone(),
    )
    .await?;

    assert_eq!(got.id, engine.state_id);

    let expected_root = engine.state.root_cell().repr_hash();
    let expected_file = engine.state.block_id().file_hash.clone();
    let expected_data = write_boc(engine.state.root_cell())?;

    assert_eq!(got.root_hash, expected_root);
    assert_eq!(got.file_hash, expected_file);
    assert_eq!(got.data, expected_data);

    Ok(())
}

#[tokio::test]
#[ignore]
async fn test_get_libraries_empty() -> Result<()> {
    struct TestEngine {
        state_id: BlockIdExt,
        state: Arc<ShardStateStuff>,
    }

    impl TestEngine {
        fn new() -> Self {
            let state = gen_master_state(
                GenMasterStateParams::default(),
                #[cfg(feature = "telemetry")]
                None,
                None,
            );
            let state_id = state.block_id().clone();
            Self { state_id, state }
        }
    }

    #[async_trait::async_trait]
    impl EngineOperations for TestEngine {
        fn load_last_applied_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
            Ok(Some(Arc::new(self.state_id.clone())))
        }
        async fn load_state(&self, id: &BlockIdExt) -> Result<Arc<ShardStateStuff>> {
            if *id == self.state_id {
                Ok(self.state.clone())
            } else {
                fail!("Wrong block ID {}", id)
            }
        }
        fn now(&self) -> u32 {
            1_640_995_200
        }
    }

    let engine = Arc::new(TestEngine::new());
    let h1 = UInt256::from([1u8; 32]);
    let h2 = UInt256::from([2u8; 32]);

    let out = LiteServerQuerySubscriber::get_libraries(
        &(engine.clone() as Arc<dyn EngineOperations>),
        vec![h1.clone(), h2.clone()],
    )
    .await?;

    assert_eq!(out.result.len(), 2);
    assert_eq!(out.result[0].hash, h1);
    assert!(out.result[0].data.is_empty());
    assert_eq!(out.result[1].hash, h2);
    assert!(out.result[1].data.is_empty());

    Ok(())
}

// #[tokio::test]
// async fn test_get_all_shards_info_simple() -> Result<()> {
//     let mut engine = LiteServerTestEngine::new().await;

//     let block_id_template = BlockIdExt {
//         shard_id: ShardIdent::masterchain(),
//         seq_no: 100,
//         root_hash: UInt256::default(),
//         file_hash: UInt256::default(),
//     };
//     let (mc_block_boc, mc_block_id) = create_minimal_block_boc(block_id_template, None)?;
//     engine.add_mock_block_with_handle(mc_block_id.clone(), mc_block_boc).await?;

//     let mc_state = gen_master_state(
//         GenMasterStateParams::default(),
//         #[cfg(feature = "telemetry")]
//         None,
//         None,
//     );
//     engine.add_ready_state(mc_block_id.clone(), mc_state);

//     engine.last_mc_block_id = mc_block_id.clone();

//     let subscriber = LiteServerQueryImpl::new(Arc::new(engine));
//     let res = subscriber.get_all_shards_info(mc_block_id.clone()).await?;

//     assert_eq!(res.id, mc_block_id);
//     assert!(!res.data.is_empty());

//     Ok(())
// }

#[ignore]
#[tokio::test]
async fn test_get_one_transaction_account_not_in_block() -> Result<()> {
    let mut engine = LiteServerTestEngine::new().await;

    let tpl = BlockIdExt {
        shard_id: ShardIdent::masterchain(),
        seq_no: 1,
        root_hash: UInt256::default(),
        file_hash: UInt256::default(),
    };
    let (boc, real_id) = create_minimal_block_boc(tpl, None)?;
    engine.add_mock_block_with_handle(real_id.clone(), boc).await?;
    let acc = AccountIdTl { workchain: 0, id: UInt256::from([2u8; 32]) };
    let engine = Arc::new(engine);

    let err = LiteServerQuerySubscriber::get_one_transaction(
        &(engine.clone() as Arc<dyn EngineOperations>),
        real_id.clone(),
        acc,
        555,
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("Transaction with lt"));
    Ok(())
}

#[ignore]
#[tokio::test]
async fn test_get_transactions_empty_list() -> Result<()> {
    let mut engine = LiteServerTestEngine::new().await;

    let tpl = BlockIdExt {
        shard_id: ShardIdent::masterchain(),
        seq_no: 2,
        root_hash: UInt256::default(),
        file_hash: UInt256::default(),
    };
    let (boc, real_id) = create_minimal_block_boc(tpl, None)?;
    engine.add_mock_block_with_handle(real_id.clone(), boc).await?;

    engine.last_mc_block_id = real_id.clone();

    let mc_state = gen_master_state(
        GenMasterStateParams::default(),
        #[cfg(feature = "telemetry")]
        None,
        None,
    );
    engine.add_ready_state(real_id.clone(), mc_state);
    let acc = AccountIdTl { workchain: 0, id: UInt256::from([3u8; 32]) };
    let engine = Arc::new(engine);

    let list = LiteServerQuerySubscriber::get_transactions(
        &(engine.clone() as Arc<dyn EngineOperations>),
        10,
        acc,
        0,
        UInt256::default(),
    )
    .await?;

    assert!(list.ids.is_empty());
    assert!(list.transactions.is_empty());
    Ok(())
}

#[tokio::test]
async fn test_send_message() -> Result<()> {
    use std::sync::{Arc, Mutex};

    struct TestSendMsgEngine {
        expected: Vec<u8>,
        seen: Arc<Mutex<usize>>,
    }
    #[async_trait::async_trait]
    impl EngineOperations for TestSendMsgEngine {
        async fn redirect_external_message(&self, message_data: &[u8]) -> Result<()> {
            assert_eq!(message_data, self.expected.as_slice());
            *self.seen.lock().unwrap() += 1;
            Ok(())
        }
        fn now(&self) -> u32 {
            0
        }
    }
    let body = vec![0xDE, 0xAD, 0xBE, 0xEF];
    let engine =
        Arc::new(TestSendMsgEngine { expected: body.clone(), seen: Arc::new(Mutex::new(0)) });

    let status = LiteServerQuerySubscriber::send_message(
        &(engine.clone() as Arc<dyn EngineOperations>),
        body,
    )
    .await
    .unwrap();

    assert_eq!(status.status, 1);
    assert_eq!(*engine.seen.lock().unwrap(), 1);
    Ok(())
}

#[tokio::test]
async fn test_send_message_engine_error_maps_to_negative_status() -> Result<()> {
    struct FailingEngine;

    #[async_trait::async_trait]
    impl EngineOperations for FailingEngine {
        async fn redirect_external_message(&self, _message_data: &[u8]) -> Result<()> {
            fail!("boom")
        }
        fn now(&self) -> u32 {
            0
        }
    }

    let engine: Arc<dyn EngineOperations> = Arc::new(FailingEngine);
    let res = LiteServerQuerySubscriber::send_message(&engine, vec![1, 2, 3]).await?;

    assert!(res.status < 0, "expected negative status on engine error; got {}", res.status);
    Ok(())
}

fn create_test_internal_message(src: &UInt256, dst: &UInt256) -> Message {
    let src_addr = MsgAddressInt::with_standart(None, 0, src.clone().into()).unwrap();
    let dst_addr = MsgAddressInt::with_standart(None, 0, dst.clone().into()).unwrap();
    let hdr = InternalMessageHeader::with_addresses(
        src_addr,
        dst_addr,
        CurrencyCollection::with_coins(100),
    );
    Message::with_int_header(hdr)
}

fn create_test_envelope(lt: u64, account_id: &UInt256) -> MsgEnvelope {
    let src = UInt256::from([0xAA; 32]);
    let msg = create_test_internal_message(&src, account_id);
    MsgEnvelope::with_routing(
        ChildCell::with_struct(&msg).unwrap(),
        100u64.into(), // fwd_fee_remaining
        IntermediateAddress::full_dest(),
        IntermediateAddress::full_dest(),
        lt,
        None,
    )
}

fn create_dispatch_queue_with_messages(
    accounts_with_messages: Vec<(UInt256, Vec<u64>)>,
) -> DispatchQueue {
    let mut dispatch_queue = DispatchQueue::default();

    for (account_id, lts) in accounts_with_messages {
        let mut account_queue = AccountDispatchQueue::default();
        for lt in lts {
            let envelope = create_test_envelope(lt, &account_id);
            let enq = EnqueuedMsg::with_param(lt, &envelope).unwrap();
            account_queue.insert(lt, &enq).unwrap();
        }

        let key: AccountId = (&account_id).into();
        dispatch_queue.set_augmentable(&key, &account_queue).unwrap();
    }

    dispatch_queue
}

fn create_out_msg_queue_with_size(size: usize) -> OutMsgQueue {
    let mut queue = OutMsgQueue::default();
    for i in 0..size {
        let hash = UInt256::from([i as u8; 32]);
        let dst = UInt256::from([(i + 1) as u8; 32]);
        let key = OutMsgQueueKey::with_workchain_id_and_prefix(0, i as u64, hash.clone());
        let envelope = create_test_envelope(i as u64, &dst);
        let enq = EnqueuedMsg::with_param(i as u64, &envelope).unwrap();
        queue.set(&key, &enq, &(i as u64)).unwrap();
    }
    queue
}

struct DispatchQueueTestEngine {
    state_id: BlockIdExt,
    state: Arc<ShardStateStuff>,
    zerostate_id: BlockIdExt,
}

impl DispatchQueueTestEngine {
    fn new(dispatch_queue: DispatchQueue, out_queue_size: usize) -> Self {
        let zerostate_id = BlockIdExt {
            shard_id: ShardIdent::masterchain(),
            seq_no: 0,
            root_hash: UInt256::from([1u8; 32]),
            file_hash: UInt256::from([2u8; 32]),
        };

        let extra = OutMsgQueueExtra { dispatch_queue, out_queue_size };
        let out_queue = create_out_msg_queue_with_size(out_queue_size);
        let queue_info = OutMsgQueueInfo::with_params(out_queue, Default::default(), extra);

        let mut ss = ShardStateUnsplit::with_ident(ShardIdent::full(0));
        ss.set_seq_no(100);
        ss.write_out_msg_queue_info(&queue_info).unwrap();

        let cell = ss.serialize().unwrap();
        let bytes = write_boc(&cell).unwrap();
        let root_hash = cell.repr_hash();
        let file_hash = UInt256::calc_file_hash(&bytes);

        let state_id =
            BlockIdExt { shard_id: ShardIdent::full(0), seq_no: 100, root_hash, file_hash };

        #[cfg(feature = "telemetry")]
        let telemetry = create_engine_telemetry();
        let allocated = create_engine_allocated();

        let state = ShardStateStuff::deserialize_state(
            state_id.clone(),
            &bytes,
            #[cfg(feature = "telemetry")]
            &telemetry,
            &allocated,
        )
        .unwrap();

        Self { state_id, state, zerostate_id }
    }
}

#[async_trait::async_trait]
impl EngineOperations for DispatchQueueTestEngine {
    fn zerostate_id(&self) -> Result<&BlockIdExt> {
        Ok(&self.zerostate_id)
    }

    fn now(&self) -> u32 {
        1640995200
    }

    async fn load_state(&self, id: &BlockIdExt) -> Result<Arc<ShardStateStuff>> {
        if *id == self.state_id {
            Ok(self.state.clone())
        } else {
            fail!("State not found for {}", id)
        }
    }

    fn load_last_applied_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
        Ok(Some(Arc::new(self.state_id.clone())))
    }
}

#[tokio::test]
async fn test_get_dispatch_queue_messages_empty() -> Result<()> {
    let dispatch_queue = DispatchQueue::default();
    let engine = Arc::new(DispatchQueueTestEngine::new(dispatch_queue, 0));

    let result = LiteServerQuerySubscriber::get_dispatch_queue_messages(
        &(engine.clone() as Arc<dyn EngineOperations>),
        0,
        engine.state_id.clone(),
        UInt256::default(),
        0,
        10,
    )
    .await?;

    assert!(result.messages.is_empty());
    assert!(bool::from(result.complete));
    Ok(())
}

#[tokio::test]
async fn test_get_dispatch_queue_messages_single_account() -> Result<()> {
    let account_id = UInt256::from([0x42u8; 32]);
    let dispatch_queue =
        create_dispatch_queue_with_messages(vec![(account_id.clone(), vec![100, 200, 300])]);

    let engine = Arc::new(DispatchQueueTestEngine::new(dispatch_queue, 0));

    let result = LiteServerQuerySubscriber::get_dispatch_queue_messages(
        &(engine.clone() as Arc<dyn EngineOperations>),
        0,
        engine.state_id.clone(),
        UInt256::default(),
        0,
        10,
    )
    .await?;

    assert_eq!(result.messages.len(), 3);
    assert_eq!(result.messages[0].lt, 100);
    assert_eq!(result.messages[1].lt, 200);
    assert_eq!(result.messages[2].lt, 300);
    assert!(bool::from(result.complete));
    Ok(())
}

#[tokio::test]
async fn test_get_dispatch_queue_messages_multiple_accounts() -> Result<()> {
    let account1 = UInt256::from([0x10u8; 32]);
    let account2 = UInt256::from([0x20u8; 32]);
    let account3 = UInt256::from([0x30u8; 32]);

    let dispatch_queue = create_dispatch_queue_with_messages(vec![
        (account1.clone(), vec![100, 200]),
        (account2.clone(), vec![150]),
        (account3.clone(), vec![50, 250]),
    ]);

    let engine = Arc::new(DispatchQueueTestEngine::new(dispatch_queue, 0));

    let result = LiteServerQuerySubscriber::get_dispatch_queue_messages(
        &(engine.clone() as Arc<dyn EngineOperations>),
        0,
        engine.state_id.clone(),
        UInt256::default(),
        0,
        10,
    )
    .await?;

    assert_eq!(result.messages.len(), 5);
    assert!(bool::from(result.complete));
    Ok(())
}

#[tokio::test]
async fn test_get_dispatch_queue_messages_with_limit() -> Result<()> {
    let account_id = UInt256::from([0x42u8; 32]);
    let dispatch_queue = create_dispatch_queue_with_messages(vec![(
        account_id.clone(),
        vec![100, 200, 300, 400, 500],
    )]);

    let engine = Arc::new(DispatchQueueTestEngine::new(dispatch_queue, 0));

    let result = LiteServerQuerySubscriber::get_dispatch_queue_messages(
        &(engine.clone() as Arc<dyn EngineOperations>),
        0,
        engine.state_id.clone(),
        UInt256::default(),
        0,
        3,
    )
    .await?;

    assert_eq!(result.messages.len(), 3);
    assert!(!bool::from(result.complete));
    Ok(())
}

#[tokio::test]
async fn test_get_dispatch_queue_messages_one_account_mode() -> Result<()> {
    let account1 = UInt256::from([0x10u8; 32]);
    let account2 = UInt256::from([0x20u8; 32]);

    let dispatch_queue = create_dispatch_queue_with_messages(vec![
        (account1.clone(), vec![100, 200]),
        (account2.clone(), vec![150, 250]),
    ]);

    let engine = Arc::new(DispatchQueueTestEngine::new(dispatch_queue, 0));

    let result = LiteServerQuerySubscriber::get_dispatch_queue_messages(
        &(engine.clone() as Arc<dyn EngineOperations>),
        0x2, // one_account mode
        engine.state_id.clone(),
        account1.clone(),
        0,
        10,
    )
    .await?;

    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.messages[0].addr, account1);
    assert_eq!(result.messages[1].addr, account1);
    assert!(bool::from(result.complete));
    Ok(())
}

#[tokio::test]
async fn test_get_dispatch_queue_messages_after_lt() -> Result<()> {
    let account_id = UInt256::from([0x42u8; 32]);
    let dispatch_queue =
        create_dispatch_queue_with_messages(vec![(account_id.clone(), vec![100, 200, 300, 400])]);

    let engine = Arc::new(DispatchQueueTestEngine::new(dispatch_queue, 0));

    let result = LiteServerQuerySubscriber::get_dispatch_queue_messages(
        &(engine.clone() as Arc<dyn EngineOperations>),
        0, // mode
        engine.state_id.clone(),
        account_id.clone(),
        200, // after_lt = 200, should skip 100 and 200
        10,
    )
    .await?;

    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.messages[0].lt, 300);
    assert_eq!(result.messages[1].lt, 400);
    assert!(bool::from(result.complete));
    Ok(())
}

struct OutMsgQueueSizesTestEngine {
    mc_state: Arc<ShardStateStuff>,
    shard_state: Arc<ShardStateStuff>,
}

impl OutMsgQueueSizesTestEngine {
    fn new(out_queue_size: usize) -> Self {
        let extra = OutMsgQueueExtra { dispatch_queue: DispatchQueue::default(), out_queue_size };
        let out_queue = create_out_msg_queue_with_size(out_queue_size);
        let queue_info = OutMsgQueueInfo::with_params(out_queue, Default::default(), extra);

        let mut shard_ss = ShardStateUnsplit::with_ident(ShardIdent::full(0));
        shard_ss.set_seq_no(100);
        shard_ss.write_out_msg_queue_info(&queue_info).unwrap();

        let shard_cell = shard_ss.serialize().unwrap();
        let shard_bytes = write_boc(&shard_cell).unwrap();
        let shard_root_hash = shard_cell.repr_hash();
        let shard_file_hash = UInt256::calc_file_hash(&shard_bytes);

        let shard_state_id = BlockIdExt {
            shard_id: ShardIdent::full(0),
            seq_no: 100,
            root_hash: shard_root_hash.clone(),
            file_hash: shard_file_hash.clone(),
        };

        #[cfg(feature = "telemetry")]
        let telemetry = create_engine_telemetry();
        let allocated = create_engine_allocated();

        let shard_state = ShardStateStuff::deserialize_state(
            shard_state_id.clone(),
            &shard_bytes,
            #[cfg(feature = "telemetry")]
            &telemetry,
            &allocated,
        )
        .unwrap();

        let mc_state = gen_master_state(
            GenMasterStateParams {
                master_state_id: Some(BlockIdExt {
                    shard_id: ShardIdent::masterchain(),
                    seq_no: 50,
                    root_hash: UInt256::from([3u8; 32]),
                    file_hash: UInt256::from([4u8; 32]),
                }),
                shard_state_id: Some(shard_state_id),
                ..Default::default()
            },
            #[cfg(feature = "telemetry")]
            None,
            None,
        );

        Self { mc_state, shard_state }
    }
}

#[async_trait::async_trait]
impl EngineOperations for OutMsgQueueSizesTestEngine {
    fn load_last_applied_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
        Ok(Some(Arc::new(self.mc_state.block_id().clone())))
    }
    fn load_shard_client_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
        Ok(None)
    }
    async fn load_state(&self, id: &BlockIdExt) -> Result<Arc<ShardStateStuff>> {
        if id.shard().is_masterchain() {
            Ok(self.mc_state.clone())
        } else {
            Ok(self.shard_state.clone())
        }
    }
}

#[tokio::test]
async fn test_get_out_msg_queue_sizes_with_cached_size() -> Result<()> {
    let engine = Arc::new(OutMsgQueueSizesTestEngine::new(42));

    let result = LiteServerQuerySubscriber::get_out_msg_queue_sizes(
        &(engine as Arc<dyn EngineOperations>),
        0,
        None,
        None,
    )
    .await?;

    assert_eq!(result.shards.len(), 2);

    let basechain_shard = result.shards.iter().find(|s| !s.id.shard().is_masterchain());
    assert!(basechain_shard.is_some());
    assert_eq!(basechain_shard.unwrap().size, 42);

    Ok(())
}

#[tokio::test]
async fn test_get_out_msg_queue_sizes_with_len_fallback() -> Result<()> {
    // out_queue_size = 0 means we use len() fallback
    let engine = Arc::new(OutMsgQueueSizesTestEngine::new(0));

    let result = LiteServerQuerySubscriber::get_out_msg_queue_sizes(
        &(engine as Arc<dyn EngineOperations>),
        0,
        None,
        None,
    )
    .await?;

    // Should have 2 shards: masterchain + basechain
    assert_eq!(result.shards.len(), 2);

    // Size should be 0 (empty queue)
    let basechain_shard = result.shards.iter().find(|s| !s.id.shard().is_masterchain());
    assert!(basechain_shard.is_some());
    assert_eq!(basechain_shard.unwrap().size, 0);

    Ok(())
}

struct ConfigParamsTestEngine {
    state_id: BlockIdExt,
    state: Arc<ShardStateStuff>,
    zerostate_id: BlockIdExt,
}

impl ConfigParamsTestEngine {
    fn new_zerostate() -> Self {
        let state = gen_master_state(
            GenMasterStateParams::default(),
            #[cfg(feature = "telemetry")]
            None,
            None,
        );

        let actual_id = state.block_id().clone();

        Self { state_id: actual_id.clone(), state, zerostate_id: actual_id }
    }
}

#[async_trait::async_trait]
impl EngineOperations for ConfigParamsTestEngine {
    fn zerostate_id(&self) -> Result<&BlockIdExt> {
        Ok(&self.zerostate_id)
    }

    async fn load_state(&self, id: &BlockIdExt) -> Result<Arc<ShardStateStuff>> {
        if id.seq_no() == self.state_id.seq_no() && id.shard() == self.state_id.shard() {
            Ok(self.state.clone())
        } else {
            fail!("State not found for {}", id)
        }
    }

    fn load_last_applied_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
        Ok(Some(Arc::new(self.state_id.clone())))
    }

    fn load_shard_client_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
        Ok(Some(Arc::new(self.state_id.clone())))
    }
}

/// Test that get_all_config_params returns empty state_proof for zerostate (seqno = 0)
#[tokio::test]
async fn test_get_all_config_params_zerostate() -> Result<()> {
    let engine = Arc::new(ConfigParamsTestEngine::new_zerostate());

    let result = LiteServerQuerySubscriber::get_config_params(
        &(engine.clone() as Arc<dyn EngineOperations>),
        CFG_VISIT_ROOT,
        engine.state_id.clone(),
        Vec::new(),
    )
    .await?;

    // For zerostate (seqno = 0), state_proof should be empty
    assert!(
        result.state_proof.is_empty(),
        "state_proof should be empty for zerostate, got {} bytes",
        result.state_proof.len()
    );
    assert!(!result.config_proof.is_empty(), "config_proof should not be empty");

    Ok(())
}

/// Build a masterchain key block at `seq_no` whose McBlockExtra carries `config`.
fn create_key_block_with_config(
    seq_no: u32,
    config: ConfigParams,
) -> Result<(Vec<u8>, BlockIdExt)> {
    let mut info = BlockInfo::default();
    info.set_shard(ShardIdent::masterchain());
    info.set_seq_no(seq_no)?;
    info.set_key_block(true);
    info.set_prev_stuff(
        false,
        &BlkPrevInfo::Block {
            prev: ExtBlkRef {
                end_lt: 1,
                seq_no: seq_no.saturating_sub(1),
                root_hash: UInt256::from([0xAB; 32]),
                file_hash: UInt256::from([0xCD; 32]),
            },
        },
    )?;

    let mut mc_extra = McBlockExtra::default();
    mc_extra.set_config(config);

    let mut extra = BlockExtra::default();
    extra.write_custom(&mc_extra)?;

    let block = Block::with_params(0, info, ValueFlow::default(), MerkleUpdate::default(), extra)?;
    finalize_block_to_boc(
        BlockIdExt::with_params(
            ShardIdent::masterchain(),
            seq_no,
            UInt256::default(),
            UInt256::default(),
        ),
        &block,
    )
}

/// Test CFG_FROM_PREV_KEY_BLOCK flag with a real (non-zerostate) chain:
/// state at seqno 20 has prev_blocks referencing key block at seqno 5.
/// Blocks after key block resolve to it; blocks before it fail.
#[tokio::test]
async fn test_get_config_params_from_prev_key_block() -> Result<()> {
    let mut cfg = ConfigParams::default();
    cfg.set_config(ConfigParamEnum::ConfigParam0(ConfigParam0 {
        config_addr: AccountId::from([0x55; 32]),
    }))?;
    let (key_block_data, key_block_id) = create_key_block_with_config(5, cfg)?;

    // prev_blocks: zerostate(0), regular(2), key(5), regular(15)
    let mut prev_blocks = OldMcBlocksInfo::default();
    let add_block = |prev_blocks: &mut OldMcBlocksInfo,
                     seq_no: u32,
                     key: bool,
                     rh: Option<UInt256>,
                     fh: Option<UInt256>| {
        let rh = rh.unwrap_or(UInt256::from([seq_no as u8; 32]));
        let fh = fh.unwrap_or(UInt256::from([seq_no as u8 | 0x80; 32]));
        prev_blocks.set_augmentable(
            &seq_no,
            &KeyExtBlkRef {
                key,
                blk_ref: ExtBlkRef {
                    seq_no,
                    end_lt: seq_no as u64 * 10000,
                    root_hash: rh,
                    file_hash: fh,
                },
            },
        )
    };
    add_block(&mut prev_blocks, 0, false, None, None)?;
    add_block(&mut prev_blocks, 2, false, None, None)?;

    // Last known state is key block state
    let state = gen_master_state(
        GenMasterStateParams {
            master_state_id: Some(key_block_id.clone()),
            prev_blocks: Some(prev_blocks.clone()),
            after_key_block: true,
            ..Default::default()
        },
        #[cfg(feature = "telemetry")]
        None,
        None,
    );

    let mut engine = LiteServerTestEngine::new().await;
    engine.add_mock_block_with_handle(key_block_id.clone(), key_block_data.clone()).await?;
    engine.set_last_mc_block_id(state.block_id().clone());
    engine.add_ready_state(state.block_id().clone(), state);
    let engine = Arc::new(engine);

    let get_cfg = |engine: &Arc<LiteServerTestEngine>, id: BlockIdExt| {
        let engine = engine.clone() as Arc<dyn EngineOperations>;
        async move {
            LiteServerQuerySubscriber::get_config_params(
                &engine,
                CFG_FROM_PREV_KEY_BLOCK | CFG_VISIT_ROOT,
                id,
                Vec::new(),
            )
            .await
        }
    };

    // Key block → should resolve to itself
    let result = get_cfg(&engine, key_block_id.clone()).await?;
    assert_eq!(result.id, key_block_id);

    // Add blocks after key block
    add_block(
        &mut prev_blocks,
        5,
        true,
        Some(key_block_id.root_hash.clone()),
        Some(key_block_id.file_hash.clone()),
    )?;
    add_block(&mut prev_blocks, 15, false, None, None)?;

    let mc_id = |seq_no: u32| {
        BlockIdExt::with_params(
            ShardIdent::masterchain(),
            seq_no,
            UInt256::from([seq_no as u8; 32]),
            UInt256::from([seq_no as u8 | 0x80; 32]),
        )
    };

    let state_id = mc_id(20);
    let state = gen_master_state(
        GenMasterStateParams {
            master_state_id: Some(state_id.clone()),
            prev_blocks: Some(prev_blocks),
            ..Default::default()
        },
        #[cfg(feature = "telemetry")]
        None,
        None,
    );

    let mut engine = LiteServerTestEngine::new().await;
    engine.add_mock_block_with_handle(key_block_id.clone(), key_block_data).await?;
    engine.set_last_mc_block_id(state.block_id().clone());
    engine.add_ready_state(state.block_id().clone(), state);
    let engine = Arc::new(engine);

    // Block after key block → should resolve to key block at seqno 5
    let result = get_cfg(&engine, mc_id(15)).await?;
    assert_eq!(result.id.seq_no(), 5);
    assert_eq!(result.id.root_hash, key_block_id.root_hash);
    assert!(result.state_proof.is_empty());
    assert!(!result.config_proof.is_empty());
    assert_eq!(result.mode, CFG_FROM_PREV_KEY_BLOCK);

    // Last known block → the same
    let result = get_cfg(&engine, state_id).await?;
    assert_eq!(result.id.seq_no(), 5);

    // Key block → should resolve to itself
    let result = get_cfg(&engine, key_block_id.clone()).await?;
    assert_eq!(result.id, key_block_id);

    // Block before key block → no previous key block, must fail
    assert!(dbg!(get_cfg(&engine, mc_id(2)).await).is_err(), "no key block before seqno 2");

    // Zerostate → no previous key block, must fail
    assert!(dbg!(get_cfg(&engine, mc_id(0)).await).is_err(), "no key block before zerostate");

    Ok(())
}

// ---------------------------------------------------------------------------
// WaitRegistry tests
// ---------------------------------------------------------------------------

struct WaitRegistryTestEngine {
    current_time: u32,
    internal_db: Arc<InternalDb>,
    tip_seqno: std::sync::atomic::AtomicU32,
    blocks: HashMap<BlockIdExt, Vec<u8>>,
    handles: HashMap<BlockIdExt, Arc<BlockHandle>>,
    /// Test signals new blocks through this channel.
    /// Value = (new block handle, new block data).
    block_rx: tokio::sync::watch::Receiver<Option<(Arc<BlockHandle>, Vec<u8>, BlockIdExt)>>,
    #[cfg(feature = "telemetry")]
    telemetry: Arc<EngineTelemetry>,
    allocated: Arc<EngineAlloc>,
}

impl WaitRegistryTestEngine {
    async fn new(
        db_suffix: &str,
    ) -> (Self, tokio::sync::watch::Sender<Option<(Arc<BlockHandle>, Vec<u8>, BlockIdExt)>>) {
        let db_path = format!("{}_wait_registry_{}", DB_PATH, db_suffix);
        fs::remove_dir_all(&db_path).ok();
        let db_config = InternalDbConfig { db_directory: db_path, ..Default::default() };
        let db = InternalDb::with_update(
            db_config,
            false,
            false,
            false,
            &|| Ok(()),
            None,
            Arc::new(AtomicU8::new(0)),
            None,
            #[cfg(feature = "telemetry")]
            create_engine_telemetry(),
            create_engine_allocated(),
        )
        .await
        .unwrap();

        let (block_tx, block_rx) = tokio::sync::watch::channel(None);

        // Create initial mc block at seqno 10
        let initial_seqno = 10u32;
        let tpl = BlockIdExt {
            shard_id: ShardIdent::masterchain(),
            seq_no: initial_seqno,
            root_hash: UInt256::default(),
            file_hash: UInt256::default(),
        };
        let (data, block_id) = create_minimal_block_boc(tpl, None).unwrap();
        let db = Arc::new(db);
        let cell = read_single_root_boc(&data).unwrap();
        let block = Block::construct_from_cell(cell).unwrap();
        let handle = db
            .create_or_load_block_handle(&block_id, Some(&block), Some(1640995200), None)
            .unwrap()
            ._to_created()
            .unwrap();
        handle.set_data();

        let mut blocks = HashMap::new();
        let mut handles = HashMap::new();
        blocks.insert(block_id.clone(), data);
        handles.insert(block_id, handle);

        let engine = Self {
            current_time: 1640995200,
            internal_db: db,
            tip_seqno: std::sync::atomic::AtomicU32::new(initial_seqno),
            blocks,
            handles,
            block_rx,
            #[cfg(feature = "telemetry")]
            telemetry: create_engine_telemetry(),
            allocated: create_engine_allocated(),
        };
        (engine, block_tx)
    }

    /// Create a block at `seqno`, register it, and return data for signaling.
    fn prepare_block(&mut self, seqno: u32) -> (Arc<BlockHandle>, Vec<u8>, BlockIdExt) {
        let tpl = BlockIdExt {
            shard_id: ShardIdent::masterchain(),
            seq_no: seqno,
            root_hash: UInt256::default(),
            file_hash: UInt256::default(),
        };
        let (data, block_id) = create_minimal_block_boc(tpl, None).unwrap();
        let cell = read_single_root_boc(&data).unwrap();
        let block = Block::construct_from_cell(cell).unwrap();
        let handle = self
            .internal_db
            .create_or_load_block_handle(&block_id, Some(&block), Some(self.current_time), None)
            .unwrap()
            ._to_created()
            .unwrap();
        handle.set_data();
        self.blocks.insert(block_id.clone(), data.clone());
        self.handles.insert(block_id.clone(), handle.clone());
        (handle, data, block_id)
    }
}

#[async_trait::async_trait]
impl EngineOperations for WaitRegistryTestEngine {
    #[cfg(feature = "telemetry")]
    fn engine_telemetry(&self) -> &Arc<EngineTelemetry> {
        &self.telemetry
    }

    fn engine_allocated(&self) -> &Arc<EngineAlloc> {
        &self.allocated
    }

    fn now(&self) -> u32 {
        self.current_time
    }

    fn load_last_applied_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
        let seqno = self.tip_seqno.load(Ordering::Relaxed);
        let id = self.handles.keys().find(|id| id.seq_no == seqno).cloned().unwrap();
        Ok(Some(Arc::new(id)))
    }

    fn load_shard_client_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
        Ok(None)
    }

    fn load_block_handle(&self, id: &BlockIdExt) -> Result<Option<Arc<BlockHandle>>> {
        Ok(self.handles.get(id).cloned())
    }

    async fn load_block(&self, handle: &BlockHandle) -> Result<BlockStuff> {
        let id = handle.id();
        let data = self.blocks.get(id).ok_or_else(|| error!("block not found: {id}"))?;
        BlockStuff::deserialize_block(id.clone(), Arc::new(data.clone()))
    }

    async fn wait_next_applied_mc_block(
        &self,
        _prev_handle: &BlockHandle,
        timeout_ms: Option<u64>,
    ) -> Result<(Arc<BlockHandle>, BlockStuff)> {
        let mut rx = self.block_rx.clone();
        let timeout = Duration::from_millis(timeout_ms.unwrap_or(10_000));
        match tokio::time::timeout(timeout, rx.changed()).await {
            Ok(Ok(())) => {
                let val = rx.borrow().clone();
                if let Some((handle, data, id)) = val {
                    let stuff = BlockStuff::deserialize_block(id, Arc::new(data))?;
                    Ok((handle, stuff))
                } else {
                    fail!("watch channel sent None")
                }
            }
            Ok(Err(_)) => fail!("watch channel closed"),
            Err(_) => fail!(NodeError::Timeout("wait_next_applied_mc_block timeout".to_string())),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_wait_registry_dispatch() -> Result<()> {
    use ton_api::ton::rpc::lite_server::{QueryPrefix, WaitMasterchainSeqno};

    let (mut engine, block_tx) = WaitRegistryTestEngine::new("dispatch").await;
    let target_seqno = 11u32; // one beyond current (10)

    // Prepare the block that will satisfy the wait
    let (new_handle, new_data, new_id) = engine.prepare_block(target_seqno);

    let engine = Arc::new(engine);
    let runtime = tokio::runtime::Handle::current();
    let subscriber = LiteServerQuerySubscriber::new(runtime, engine.clone(), 100, 100, 1)?;

    // Build wait-prefixed GetTime query targeting seqno 11
    let inner_query = serialize_boxed(&GetTime.into_tl_object())?;
    let mut data = Vec::new();
    data.extend_from_slice(&QueryPrefix::constructor_const().to_le_bytes());
    data.extend_from_slice(&WaitMasterchainSeqno::constructor_const().to_le_bytes());
    data.extend_from_slice(&(target_seqno as i32).to_le_bytes());
    data.extend_from_slice(&5000i32.to_le_bytes()); // timeout_ms
    data.extend_from_slice(&inner_query);

    // Submit 50 queries — all should go into the wait registry
    let mut handles = Vec::new();
    for _ in 0..50 {
        let query = Query { data: data.clone().into() };
        let reply = subscriber
            .try_consume_query(
                query.into_tl_object(),
                &AdnlPeers::with_keys(KeyId::from_data([0u8; 32]), KeyId::from_data([0u8; 32])),
            )
            .await?;
        match reply {
            QueryResult::Consumed(QueryAnswer::Pending(handle)) => {
                handles.push(handle);
            }
            other => fail!("Expected Pending, got {:?}", std::mem::discriminant(&other)),
        }
    }
    assert_eq!(handles.len(), 50);

    // Signal the new block — watcher should dispatch all queries
    engine.tip_seqno.store(target_seqno, Ordering::Relaxed);
    block_tx.send(Some((new_handle, new_data, new_id))).unwrap();

    // All queries should complete with GetTime response
    let start = std::time::Instant::now();
    for handle in handles {
        let result = handle.await??;
        assert!(result.answer.is_some(), "expected answer from dispatched query");
    }
    let elapsed = start.elapsed().as_millis();
    println!("wait_registry_dispatch: 50 queries completed in {elapsed}ms");
    assert!(elapsed < 2000, "queries should complete quickly after block signal, got {elapsed}ms");

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_wait_registry_timeout() -> Result<()> {
    use ton_api::ton::rpc::lite_server::{QueryPrefix, WaitMasterchainSeqno};

    let (engine, _block_tx) = WaitRegistryTestEngine::new("timeout").await;
    let engine = Arc::new(engine);
    let runtime = tokio::runtime::Handle::current();
    let subscriber = LiteServerQuerySubscriber::new(runtime, engine.clone(), 100, 100, 1)?;

    // Build wait-prefixed GetTime query targeting seqno 99 (unreachable) with 500ms timeout
    let inner_query = serialize_boxed(&GetTime.into_tl_object())?;
    let target_seqno = 99u32;
    let timeout_ms = 500i32;
    let mut data = Vec::new();
    data.extend_from_slice(&QueryPrefix::constructor_const().to_le_bytes());
    data.extend_from_slice(&WaitMasterchainSeqno::constructor_const().to_le_bytes());
    data.extend_from_slice(&(target_seqno as i32).to_le_bytes());
    data.extend_from_slice(&timeout_ms.to_le_bytes());
    data.extend_from_slice(&inner_query);

    // Submit 10 queries
    let mut handles = Vec::new();
    for _ in 0..10 {
        let query = Query { data: data.clone().into() };
        let reply = subscriber
            .try_consume_query(
                query.into_tl_object(),
                &AdnlPeers::with_keys(KeyId::from_data([0u8; 32]), KeyId::from_data([0u8; 32])),
            )
            .await?;
        match reply {
            QueryResult::Consumed(QueryAnswer::Pending(handle)) => {
                handles.push(handle);
            }
            other => fail!("Expected Pending, got {:?}", std::mem::discriminant(&other)),
        }
    }

    // Wait for timeout — all queries should get LITE_SERVER_TIMEOUT
    let start = std::time::Instant::now();
    for handle in handles {
        let result = handle.await??;
        let answer = result.answer.expect("expected timeout answer");
        match answer {
            Answer::Object(obj) => {
                let debug = format!("{:?}", obj.object);
                assert!(debug.contains("TIMEOUT"), "expected TIMEOUT error, got: {debug}",);
            }
            _ => fail!("expected Object answer for timeout"),
        }
    }
    let elapsed = start.elapsed().as_millis();
    println!("wait_registry_timeout: 10 queries timed out in {elapsed}ms");
    assert!(elapsed >= 400 && elapsed < 3000, "timeout should be ~500ms, got {elapsed}ms");

    Ok(())
}

/// Short-timeout query arrives while watcher sleeps on a long-timeout query.
/// The short query must expire at its own deadline (~300ms), not at the long one (~5s).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_wait_registry_mixed_timeouts() -> Result<()> {
    use ton_api::ton::rpc::lite_server::{QueryPrefix, WaitMasterchainSeqno};

    let (engine, _block_tx) = WaitRegistryTestEngine::new("mixed").await;
    let engine = Arc::new(engine);
    let runtime = tokio::runtime::Handle::current();
    let subscriber = LiteServerQuerySubscriber::new(runtime, engine.clone(), 100, 100, 1)?;

    let inner_query = serialize_boxed(&GetTime.into_tl_object())?;
    let target_seqno = 99u32; // unreachable

    // Helper to build a wait-prefixed query with given timeout
    let build_query = |timeout_ms: i32| -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&QueryPrefix::constructor_const().to_le_bytes());
        data.extend_from_slice(&WaitMasterchainSeqno::constructor_const().to_le_bytes());
        data.extend_from_slice(&(target_seqno as i32).to_le_bytes());
        data.extend_from_slice(&timeout_ms.to_le_bytes());
        data.extend_from_slice(&inner_query);
        data
    };

    // 1. Submit a long-timeout query (5s) — watcher starts and sleeps
    let long_data = build_query(5000);
    let query = Query { data: long_data.into() };
    let long_handle = match subscriber
        .try_consume_query(
            query.into_tl_object(),
            &AdnlPeers::with_keys(KeyId::from_data([0u8; 32]), KeyId::from_data([0u8; 32])),
        )
        .await?
    {
        QueryResult::Consumed(QueryAnswer::Pending(h)) => h,
        _ => fail!("expected Pending"),
    };

    // Give watcher time to start and enter wait_next_applied_mc_block
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 2. Submit a short-timeout query (300ms) — should wake the watcher
    let short_data = build_query(300);
    let query = Query { data: short_data.into() };
    let start = std::time::Instant::now();
    let short_handle = match subscriber
        .try_consume_query(
            query.into_tl_object(),
            &AdnlPeers::with_keys(KeyId::from_data([0u8; 32]), KeyId::from_data([0u8; 32])),
        )
        .await?
    {
        QueryResult::Consumed(QueryAnswer::Pending(h)) => h,
        _ => fail!("expected Pending"),
    };

    // 3. Short query should timeout at ~300ms, not ~5s
    let short_result = short_handle.await??;
    let short_elapsed = start.elapsed().as_millis();
    println!("mixed_timeouts: short query expired in {short_elapsed}ms");

    let answer = short_result.answer.expect("expected timeout answer");
    match answer {
        Answer::Object(obj) => {
            let debug = format!("{:?}", obj.object);
            assert!(debug.contains("TIMEOUT"), "expected TIMEOUT, got: {debug}");
        }
        _ => fail!("expected Object answer"),
    }
    // Must expire close to 300ms, definitely not waiting for the 5s query
    assert!(
        short_elapsed >= 200 && short_elapsed < 1000,
        "short query should timeout at ~300ms, got {short_elapsed}ms"
    );

    // 4. Long query should still be pending (not expired yet)
    //    Cancel it to avoid waiting 5s in the test
    long_handle.abort();

    Ok(())
}

/// Verify that concurrent identical GetAccountState requests are coalesced:
/// only one computation runs, the rest wait and receive the shared result.
///
/// Synchronization: a Barrier ensures all N tasks start simultaneously, and
/// a Notify inside the operation blocks the computation until the test confirms
/// all tasks have entered do_or_wait. This makes the test deterministic and
/// independent of scheduler ordering.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_account_state_coalescing() -> Result<()> {
    use super::AccountCacheKey;
    use crate::types::awaiters_pool::AwaitersPool;
    use ton_api::ton::lite_server::accountstate::AccountState;

    let allocated = create_engine_allocated();
    #[cfg(feature = "telemetry")]
    let telemetry = create_engine_telemetry();
    let pool: Arc<AwaitersPool<AccountCacheKey, AccountState>> = Arc::new(AwaitersPool::new(
        "test_coalesce",
        #[cfg(feature = "telemetry")]
        telemetry,
        allocated,
    ));

    let key = AccountCacheKey {
        block_root_hash: UInt256::from([0xAA; 32]),
        account_hash: UInt256::from([0xBB; 32]),
        workchain: 0,
        prune: false,
    };

    let compute_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    // Counter: each task increments right before calling do_or_wait.
    // The operation blocks until this counter reaches N, guaranteeing
    // all tasks have entered do_or_wait before the result is produced.
    let entered_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let proceed = Arc::new(tokio::sync::Notify::new());
    const N: usize = 20;

    // Barrier so all tasks start at the same instant.
    let barrier = Arc::new(tokio::sync::Barrier::new(N));

    let mut handles = Vec::new();
    for _ in 0..N {
        let pool = pool.clone();
        let key = key.clone();
        let compute_count = compute_count.clone();
        let entered_count = entered_count.clone();
        let proceed = proceed.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            // All tasks synchronize here before racing into do_or_wait.
            barrier.wait().await;
            // Mark ourselves as about to enter do_or_wait.
            entered_count.fetch_add(1, Ordering::SeqCst);
            pool.do_or_wait(&key, None, {
                let compute_count = compute_count.clone();
                let proceed = proceed.clone();
                async move {
                    compute_count.fetch_add(1, Ordering::SeqCst);
                    // Wait until the test confirms all N tasks have entered.
                    proceed.notified().await;
                    Ok(AccountState {
                        id: BlockIdExt::default(),
                        shardblk: BlockIdExt::default(),
                        shard_proof: vec![1, 2, 3],
                        proof: vec![4, 5, 6],
                        state: vec![7, 8, 9],
                    })
                }
            })
            .await
        }));
    }

    // Wait until all N tasks have entered do_or_wait, then let the operation proceed.
    loop {
        if entered_count.load(Ordering::SeqCst) >= N as u32 {
            break;
        }
        tokio::task::yield_now().await;
    }
    // Small yield to ensure the last task has actually entered do_or_wait
    // (it incremented the counter right before the call).
    tokio::time::sleep(Duration::from_millis(10)).await;
    proceed.notify_one();

    let mut results = Vec::new();
    for h in handles {
        results.push(h.await.unwrap()?);
    }

    // Only one computation should have run
    let count = compute_count.load(Ordering::SeqCst);
    assert_eq!(count, 1, "expected 1 computation, got {count}");

    // All results should be identical
    for r in &results {
        let state = r.as_ref().unwrap();
        assert_eq!(state.shard_proof, vec![1, 2, 3]);
        assert_eq!(state.proof, vec![4, 5, 6]);
        assert_eq!(state.state, vec![7, 8, 9]);
    }

    Ok(())
}

/// Mirrors get_account_state_coalesced: LRU check -> do_or_wait -> LRU insert with byte tracking.
async fn do_lru_query(
    pool: &AwaitersPool<AccountCacheKey, AccountState>,
    lru: &Mutex<(u64, u64, lru::LruCache<AccountCacheKey, AccountState>)>,
    key: &AccountCacheKey,
    compute_count: &Arc<std::sync::atomic::AtomicU32>,
    make_state: impl Fn() -> AccountState,
) -> Result<(AccountState, bool)> {
    use super::account_state_byte_size;

    if let Some(state) = lru.lock().unwrap().2.get(key).cloned() {
        return Ok((state, true));
    }

    let computed = Arc::new(AtomicBool::new(false));
    let result = pool
        .do_or_wait(key, None, {
            let computed = computed.clone();
            let compute_count = compute_count.clone();
            async move {
                computed.store(true, Ordering::Relaxed);
                compute_count.fetch_add(1, Ordering::SeqCst);
                Ok(make_state())
            }
        })
        .await?;

    match result {
        Some(state) => {
            if computed.load(Ordering::Relaxed) {
                let entry_size = account_state_byte_size(&state);
                let mut cache = lru.lock().unwrap();
                if cache.2.push(key.clone(), state.clone()).is_none() {
                    cache.0 += entry_size;
                }
                while cache.0 > cache.1 {
                    if let Some((_, evicted)) = cache.2.pop_lru() {
                        cache.0 -= account_state_byte_size(&evicted);
                    } else {
                        break;
                    }
                }
            }
            Ok((state, false))
        }
        None => Err(error!("unexpected None")),
    }
}

/// Verify the LRU + coalescing integration, mirroring the real
/// get_account_state_coalesced flow: check LRU -> do_or_wait -> insert LRU.
///
/// 1. First call: LRU miss -> do_or_wait computes -> result inserted into LRU.
/// 2. Second call (same key): LRU hit -> do_or_wait never called.
/// 3. Third call (different key): LRU miss -> computes again.
#[tokio::test]
async fn test_account_state_lru_cache() -> Result<()> {
    let allocated = create_engine_allocated();
    #[cfg(feature = "telemetry")]
    let telemetry = create_engine_telemetry();
    let pool: AwaitersPool<AccountCacheKey, AccountState> = AwaitersPool::new(
        "test_lru",
        #[cfg(feature = "telemetry")]
        telemetry,
        allocated,
    );
    // Byte-limited LRU: (total_bytes, max_bytes, cache)
    let max_bytes: u64 = 1024 * 1024; // 1 MB, plenty for test
    let lru: Mutex<(u64, u64, lru::LruCache<AccountCacheKey, AccountState>)> =
        Mutex::new((0u64, max_bytes, lru::LruCache::unbounded()));

    let compute_count = Arc::new(std::sync::atomic::AtomicU32::new(0));

    let key_a = AccountCacheKey {
        block_root_hash: UInt256::from([0xAA; 32]),
        account_hash: UInt256::from([0xBB; 32]),
        workchain: 0,
        prune: false,
    };
    let key_b = AccountCacheKey {
        block_root_hash: UInt256::from([0xCC; 32]),
        account_hash: UInt256::from([0xDD; 32]),
        workchain: 0,
        prune: false,
    };

    let make_state = |tag: u8| AccountState {
        id: BlockIdExt::default(),
        shardblk: BlockIdExt::default(),
        shard_proof: vec![tag],
        proof: vec![tag],
        state: vec![tag],
    };

    // 1. First call with key_a -- LRU miss, do_or_wait computes
    let (state, hit) =
        do_lru_query(&pool, &lru, &key_a, &compute_count, || make_state(0xAA)).await?;
    assert!(!hit, "first call should not be a cache hit");
    assert_eq!(state.shard_proof, vec![0xAA]);
    assert_eq!(compute_count.load(Ordering::SeqCst), 1);

    // 2. Second call with key_a -- LRU hit, do_or_wait never called
    let (state, hit) =
        do_lru_query(&pool, &lru, &key_a, &compute_count, || make_state(0xAA)).await?;
    assert!(hit, "second call should be a cache hit");
    assert_eq!(state.shard_proof, vec![0xAA]);
    assert_eq!(compute_count.load(Ordering::SeqCst), 1, "no new computation expected");

    // 3. Different key -- LRU miss, computes
    let (state, hit) =
        do_lru_query(&pool, &lru, &key_b, &compute_count, || make_state(0xBB)).await?;
    assert!(!hit, "different key should not be a cache hit");
    assert_eq!(state.shard_proof, vec![0xBB]);
    assert_eq!(compute_count.load(Ordering::SeqCst), 2);

    // 4. key_a still in cache
    let (state, hit) =
        do_lru_query(&pool, &lru, &key_a, &compute_count, || make_state(0xAA)).await?;
    assert!(hit, "key_a should still be cached");
    assert_eq!(state.shard_proof, vec![0xAA]);
    assert_eq!(compute_count.load(Ordering::SeqCst), 2, "no new computation expected");

    Ok(())
}

/// Verify that byte-limited LRU eviction works: inserting entries that
/// exceed max_bytes causes the least recently used entries to be evicted.
/// Each entry has shard_proof=1, proof=1, state=1 byte => 3 bytes per entry.
/// With max_bytes=7, two entries fit (6 bytes) but three exceed the limit.
#[tokio::test]
async fn test_account_state_lru_eviction() -> Result<()> {
    let allocated = create_engine_allocated();
    #[cfg(feature = "telemetry")]
    let telemetry = create_engine_telemetry();
    let pool: AwaitersPool<AccountCacheKey, AccountState> = AwaitersPool::new(
        "test_lru_evict",
        #[cfg(feature = "telemetry")]
        telemetry,
        allocated,
    );
    // Each entry = 3 bytes (1+1+1). max_bytes=7 => 2 entries fit, 3rd triggers eviction.
    let max_bytes: u64 = 7;
    let lru: Mutex<(u64, u64, lru::LruCache<AccountCacheKey, AccountState>)> =
        Mutex::new((0u64, max_bytes, lru::LruCache::unbounded()));

    let compute_count = Arc::new(std::sync::atomic::AtomicU32::new(0));

    let make_key = |byte: u8| AccountCacheKey {
        block_root_hash: UInt256::from([byte; 32]),
        account_hash: UInt256::from([byte; 32]),
        workchain: 0,
        prune: false,
    };
    let make_state = |tag: u8| AccountState {
        id: BlockIdExt::default(),
        shardblk: BlockIdExt::default(),
        shard_proof: vec![tag],
        proof: vec![tag],
        state: vec![tag],
    };

    let key_a = make_key(0xAA);
    let key_b = make_key(0xBB);
    let key_c = make_key(0xCC);

    // Insert key_a (3 bytes), total=3, under limit=7
    let (_, hit) = do_lru_query(&pool, &lru, &key_a, &compute_count, || make_state(0xAA)).await?;
    assert!(!hit);
    assert_eq!(lru.lock().unwrap().0, 3, "total_bytes after key_a");

    // Insert key_b (3 bytes), total=6, still under limit=7
    let (_, hit) = do_lru_query(&pool, &lru, &key_b, &compute_count, || make_state(0xBB)).await?;
    assert!(!hit);
    assert_eq!(lru.lock().unwrap().0, 6, "total_bytes after key_b");
    assert_eq!(compute_count.load(Ordering::SeqCst), 2);

    // Both should be cached
    let (_, hit) = do_lru_query(&pool, &lru, &key_a, &compute_count, || make_state(0xAA)).await?;
    assert!(hit, "key_a should be cached");
    let (_, hit) = do_lru_query(&pool, &lru, &key_b, &compute_count, || make_state(0xBB)).await?;
    assert!(hit, "key_b should be cached");
    assert_eq!(compute_count.load(Ordering::SeqCst), 2, "no new computations");

    // Insert key_c (3 bytes), total would be 9 > 7, so LRU eviction kicks in.
    // key_a is LRU (key_b was accessed more recently above), so key_a is evicted.
    // After eviction: total = 9 - 3 = 6 (key_b + key_c).
    let (_, hit) = do_lru_query(&pool, &lru, &key_c, &compute_count, || make_state(0xCC)).await?;
    assert!(!hit);
    assert_eq!(compute_count.load(Ordering::SeqCst), 3);
    assert_eq!(lru.lock().unwrap().0, 6, "total_bytes after eviction");
    assert_eq!(lru.lock().unwrap().2.len(), 2, "cache should have 2 entries");

    // key_a was evicted -- should recompute
    let (state, hit) =
        do_lru_query(&pool, &lru, &key_a, &compute_count, || make_state(0xAA)).await?;
    assert!(!hit, "key_a should have been evicted");
    assert_eq!(state.shard_proof, vec![0xAA]);
    assert_eq!(compute_count.load(Ordering::SeqCst), 4, "key_a should recompute");

    // key_b was evicted by key_a re-insert (key_c + key_a now, key_b was LRU)
    let (_, hit) = do_lru_query(&pool, &lru, &key_b, &compute_count, || make_state(0xBB)).await?;
    assert!(!hit, "key_b should have been evicted");
    assert_eq!(compute_count.load(Ordering::SeqCst), 5);

    Ok(())
}

// list_block_transactions tests
fn build_block_with_txs(
    shard: ShardIdent,
    seq_no: u32,
    txs: &[(UInt256, u64)],
) -> Result<(Vec<u8>, BlockIdExt)> {
    let mut sab = ShardAccountBlocks::default();
    for (account_id, lt) in txs {
        let mut tx = Transaction::with_address_and_status(
            account_id.clone().into(),
            AccountStatus::AccStateActive,
        );
        tx.set_logical_time(*lt);
        let acc_block = AccountBlock::with_transaction(account_id.clone().into(), &tx)?;
        sab.insert(&acc_block)?;
    }
    let mut extra = BlockExtra::default();
    extra.write_account_blocks(&sab)?;

    let mut info = BlockInfo::default();
    info.set_shard(shard.clone());
    info.set_seq_no(seq_no)?;
    info.set_prev_stuff(
        false,
        &BlkPrevInfo::Block {
            prev: ExtBlkRef {
                end_lt: 1,
                seq_no: seq_no.saturating_sub(1),
                root_hash: UInt256::from([0xAB; 32]),
                file_hash: UInt256::from([0xCD; 32]),
            },
        },
    )?;
    let block = Block::with_params(0, info, ValueFlow::default(), MerkleUpdate::default(), extra)?;
    finalize_block_to_boc(
        BlockIdExt {
            shard_id: shard,
            seq_no,
            root_hash: UInt256::default(),
            file_hash: UInt256::default(),
        },
        &block,
    )
}

fn build_block_with_multi_tx_account(
    shard: ShardIdent,
    seq_no: u32,
    account_id: &UInt256,
    lts: &[u64],
) -> Result<(Vec<u8>, BlockIdExt)> {
    let account_addr: AccountId = account_id.clone().into();
    let mut transactions = Transactions::default();
    for lt in lts {
        let mut tx = Transaction::with_address_and_status(
            account_addr.clone(),
            AccountStatus::AccStateActive,
        );
        tx.set_logical_time(*lt);
        transactions.insert(&tx)?;
    }
    let hash_update = HashUpdate::with_hashes(UInt256::default(), UInt256::default());
    let acc_block = AccountBlock::with_params(&account_addr, &transactions, &hash_update)?;
    let mut sab = ShardAccountBlocks::default();
    sab.insert(&acc_block)?;

    let mut extra = BlockExtra::default();
    extra.write_account_blocks(&sab)?;

    let mut info = BlockInfo::default();
    info.set_shard(shard.clone());
    info.set_seq_no(seq_no)?;
    info.set_prev_stuff(
        false,
        &BlkPrevInfo::Block {
            prev: ExtBlkRef {
                end_lt: 1,
                seq_no: seq_no.saturating_sub(1),
                root_hash: UInt256::from([0xAB; 32]),
                file_hash: UInt256::from([0xCD; 32]),
            },
        },
    )?;
    let block = Block::with_params(0, info, ValueFlow::default(), MerkleUpdate::default(), extra)?;
    finalize_block_to_boc(
        BlockIdExt {
            shard_id: shard,
            seq_no,
            root_hash: UInt256::default(),
            file_hash: UInt256::default(),
        },
        &block,
    )
}

#[tokio::test]
async fn test_list_block_transactions_empty_block() -> Result<()> {
    let mut engine = LiteServerTestEngine::new().await;
    let shard = ShardIdent::masterchain();
    let (data, block_id) = build_block_with_txs(shard, 200, &[])?;
    engine.add_mock_block_with_handle(block_id.clone(), data).await?;
    let engine = Arc::new(engine);

    let result = LiteServerQuerySubscriber::list_block_transactions(
        &(engine.clone() as Arc<dyn EngineOperations>),
        block_id.clone(),
        0,
        100,
        None,
    )
    .await?;

    assert_eq!(result.ids.len(), 0);
    assert_eq!(result.incomplete, false.into());
    assert_eq!(result.req_count, 100);
    assert_eq!(result.id, block_id);
    Ok(())
}

#[tokio::test]
async fn test_list_block_transactions_single_tx() -> Result<()> {
    let mut engine = LiteServerTestEngine::new().await;
    let shard = ShardIdent::masterchain();
    let acc = UInt256::from([0x11; 32]);
    let (data, block_id) = build_block_with_txs(shard, 201, &[(acc.clone(), 100)])?;
    engine.add_mock_block_with_handle(block_id.clone(), data).await?;
    let engine = Arc::new(engine);

    let result = LiteServerQuerySubscriber::list_block_transactions(
        &(engine.clone() as Arc<dyn EngineOperations>),
        block_id.clone(),
        0,
        100,
        None,
    )
    .await?;

    assert_eq!(result.ids.len(), 1);
    assert_eq!(result.incomplete, false.into());
    let tid = &result.ids[0];
    assert_eq!(tid.account.as_ref().unwrap(), &acc);
    assert_eq!(tid.lt.unwrap(), 100);
    assert!(tid.hash.is_some());
    Ok(())
}

#[tokio::test]
async fn test_list_block_transactions_with_proof() -> Result<()> {
    let mut engine = LiteServerTestEngine::new().await;
    let shard = ShardIdent::masterchain();
    let txs = vec![(UInt256::from([0x11; 32]), 100)];
    let (data, block_id) = build_block_with_txs(shard, 206, &txs)?;
    engine.add_mock_block_with_handle(block_id.clone(), data).await?;
    let engine = Arc::new(engine);

    let result_no_proof = LiteServerQuerySubscriber::list_block_transactions(
        &(engine.clone() as Arc<dyn EngineOperations>),
        block_id.clone(),
        0,
        100,
        None,
    )
    .await?;
    assert!(result_no_proof.proof.is_empty());

    let result_with_proof = LiteServerQuerySubscriber::list_block_transactions(
        &(engine.clone() as Arc<dyn EngineOperations>),
        block_id.clone(),
        0x20, // WANT_PROOF
        100,
        None,
    )
    .await?;
    assert!(!result_with_proof.proof.is_empty());
    let proof_cell = read_single_root_boc(&result_with_proof.proof)?;
    let _proof = MerkleProof::construct_from_cell(proof_cell)?;
    Ok(())
}

#[tokio::test]
async fn test_list_block_transactions_ext_returns_tx_cells() -> Result<()> {
    let mut engine = LiteServerTestEngine::new().await;
    let shard = ShardIdent::masterchain();
    let txs = vec![(UInt256::from([0x11; 32]), 100), (UInt256::from([0x22; 32]), 200)];
    let (data, block_id) = build_block_with_txs(shard, 207, &txs)?;
    engine.add_mock_block_with_handle(block_id.clone(), data).await?;
    let engine = Arc::new(engine);

    let result = LiteServerQuerySubscriber::list_block_transactions_ext(
        &(engine.clone() as Arc<dyn EngineOperations>),
        block_id.clone(),
        0,
        100,
        None,
    )
    .await?;

    assert_eq!(result.id, block_id);
    assert_eq!(result.incomplete, false.into());
    assert!(!result.transactions.is_empty());
    Ok(())
}

#[tokio::test]
async fn test_list_block_transactions_multiple_txs_per_account() -> Result<()> {
    let mut engine = LiteServerTestEngine::new().await;
    let shard = ShardIdent::masterchain();
    let acc = UInt256::from([0x42; 32]);
    let (data, block_id) = build_block_with_multi_tx_account(shard, 210, &acc, &[10, 20, 30])?;
    engine.add_mock_block_with_handle(block_id.clone(), data).await?;
    let engine = Arc::new(engine);

    let result = LiteServerQuerySubscriber::list_block_transactions(
        &(engine.clone() as Arc<dyn EngineOperations>),
        block_id.clone(),
        0,
        100,
        None,
    )
    .await?;

    assert_eq!(result.ids.len(), 3);
    assert_eq!(result.ids[0].lt.unwrap(), 10);
    assert_eq!(result.ids[1].lt.unwrap(), 20);
    assert_eq!(result.ids[2].lt.unwrap(), 30);
    for tid in &result.ids {
        assert_eq!(tid.account.as_ref().unwrap(), &acc);
    }
    Ok(())
}

// VM stack BOC serialization roundtrip tests
#[test]
fn test_vm_stack_boc_roundtrip_mixed_types() {
    let empty_cell = ton_block::BuilderData::new().into_cell().unwrap();
    let cases: Vec<Vec<StackItem>> = vec![
        vec![],
        vec![StackItem::int(0)],
        vec![StackItem::int(42), StackItem::int(-1)],
        vec![StackItem::int(1), StackItem::int(2), StackItem::int(3)],
        vec![StackItem::cell(empty_cell), StackItem::None],
    ];
    for items in &cases {
        let boc = serialize_vm_stack_boc(items).unwrap();
        let decoded = deserialize_vm_stack_boc(&boc).unwrap();
        assert_eq!(decoded.len(), items.len(), "length mismatch for {:?}", items);
    }
}

#[test]
fn test_vm_stack_boc_roundtrip_extreme_ints() {
    let items = vec![StackItem::int(i64::MAX), StackItem::int(i64::MIN)];
    let boc = serialize_vm_stack_boc(&items).unwrap();
    let decoded = deserialize_vm_stack_boc(&boc).unwrap();
    assert_eq!(decoded.len(), 2);
    match &decoded[0] {
        StackItem::Integer(v) => {
            assert_eq!(v.as_integer_value(i64::MIN..=i64::MAX).unwrap(), i64::MAX)
        }
        other => panic!("expected int(MAX), got {:?}", other),
    }
    match &decoded[1] {
        StackItem::Integer(v) => {
            assert_eq!(v.as_integer_value(i64::MIN..=i64::MAX).unwrap(), i64::MIN)
        }
        other => panic!("expected int(MIN), got {:?}", other),
    }
}
