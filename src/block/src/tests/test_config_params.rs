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
#![allow(clippy::inconsistent_digit_grouping, clippy::unusual_byte_groupings)]

use super::*;
use crate::{
    read_single_root_boc, write_read_and_assert, Ed25519KeyOption, Serializable, ValidatorDescr,
    VarUInteger32, ZeroizingBytes,
};
use rand::Rng;

fn get_config_param0() -> ConfigParam0 {
    ConfigParam0 { config_addr: AccountId::from([1; 32]) }
}

#[test]
fn test_config_param_0() {
    write_read_and_assert(get_config_param0());
}

fn get_config_param1() -> ConfigParam1 {
    ConfigParam1 { elector_addr: AccountId::from([1; 32]) }
}

#[test]
fn test_config_param_1() {
    write_read_and_assert(get_config_param1());
}

fn get_config_param16() -> ConfigParam16 {
    ConfigParam16 {
        max_validators: Number16::from(23424u16),
        max_main_validators: Number16::from(35553u16),
        min_validators: Number16::from(11u16),
    }
}

#[test]
fn test_config_param_16() {
    write_read_and_assert(get_config_param16());
}

fn get_config_param17() -> ConfigParam17 {
    ConfigParam17 {
        min_stake: Coins::zero(),
        max_stake: Coins::one(),
        min_total_stake: Coins::from(100_000_000),
        max_stake_factor: 12121,
    }
}

#[test]
fn test_config_param_17() {
    write_read_and_assert(get_config_param17());
}

fn get_storage_prices() -> StoragePrices {
    let mut rng = rand::thread_rng();
    StoragePrices {
        bit_price_ps: rng.gen(),
        cell_price_ps: rng.gen(),
        mc_bit_price_ps: rng.gen(),
        mc_cell_price_ps: rng.gen(),
        utime_since: rng.gen(),
    }
}

#[test]
fn test_config_storage_prices() {
    for _ in 0..10 {
        write_read_and_assert(get_storage_prices());
    }
}

fn get_config_param18() -> ConfigParam18 {
    let mut cp18 = ConfigParam18::default();
    for _ in 0..10 {
        cp18.insert(&get_storage_prices()).unwrap();
    }
    cp18
}

#[test]
fn test_config_param_18() {
    let mut cp18 = ConfigParam18::default();
    assert_eq!(cp18.map.len().unwrap(), 0);
    cp18.write_to_new_cell().expect_err("Empty ConfigParam18 can't be serialized");

    for i in 0..10 {
        cp18.get(i).expect_err(&format!("param with index {} must not be present yet", i));
        cp18.insert(&get_storage_prices()).unwrap();
        cp18.get(i).unwrap();
        assert_eq!(cp18.map.len().unwrap(), i as usize + 1);
        write_read_and_assert(cp18.clone());
    }

    for n in 0..10 {
        cp18.get(n).unwrap();
    }
    cp18.get(11).expect_err("param with index 11 must not be present");
}

fn get_gas_limit_prices() -> GasLimitsPrices {
    let mut rng = rand::thread_rng();
    let mut glp = GasLimitsPrices {
        gas_price: rng.gen(),
        gas_limit: rng.gen(),
        gas_credit: rng.gen(),
        block_gas_limit: rng.gen(),
        freeze_due_limit: rng.gen(),
        delete_due_limit: rng.gen(),
        special_gas_limit: rng.gen(),
        flat_gas_limit: rng.gen(),
        flat_gas_price: rng.gen(),
        max_gas_threshold: 0,
    };
    glp.max_gas_threshold = glp.calc_max_gas_threshold(glp.gas_limit);
    glp
}

#[test]
fn test_config_gas_limit_price() {
    for _ in 0..10 {
        write_read_and_assert(get_gas_limit_prices());
    }
}

fn get_msg_forward_prices() -> MsgForwardPrices {
    let mut rng = rand::thread_rng();
    MsgForwardPrices {
        lump_price: rng.gen(),
        bit_price: rng.gen(),
        cell_price: rng.gen(),
        ihr_price_factor: rng.gen(),
        first_frac: rng.gen(),
        next_frac: rng.gen(),
    }
}

#[test]
fn test_config_msg_forward_prices() {
    for _ in 0..10 {
        write_read_and_assert(get_msg_forward_prices());
    }
}

fn get_cat_chain_config() -> CatchainConfig {
    let mut rng = rand::thread_rng();
    CatchainConfig {
        shuffle_mc_validators: rng.gen(),
        isolate_mc_validators: rng.gen(),
        mc_catchain_lifetime: rng.gen(),
        shard_catchain_lifetime: rng.gen(),
        shard_validators_lifetime: rng.gen(),
        shard_validators_num: rng.gen(),
    }
}

#[test]
fn test_config_cat_chain_config() {
    for _ in 0..10 {
        write_read_and_assert(get_cat_chain_config());
    }
}

fn get_config_param31() -> ConfigParam31 {
    let mut cp31 = ConfigParam31::default();
    for _ in 1..10 {
        cp31.add_address(UInt256::rand().into());
    }
    cp31
}

#[test]
fn test_config_param_31() {
    let mut cp31 = ConfigParam31::default();
    write_read_and_assert(cp31.clone());

    assert_eq!(cp31.fundamental_smc_addr.len().unwrap(), 0);

    for n in 1..10 {
        cp31.add_address(UInt256::rand().into());
        assert_eq!(cp31.fundamental_smc_addr.len().unwrap(), n);
        write_read_and_assert(cp31.clone());
    }

    cp31.fundamental_smc_addr.iterate_keys(|_key: UInt256| Ok(true)).unwrap();
}

fn get_validator_set() -> ValidatorSet {
    let mut list = vec![];
    for n in 0..2 {
        let keypair = Ed25519KeyOption::<ZeroizingBytes>::generate().unwrap();
        let key = SigPubKey::from_bytes(keypair.pub_key().unwrap()).unwrap();
        let vd = ValidatorDescr::with_params(key, n, None);
        list.push(vd);
    }

    ValidatorSet::new(0, 100, 1, list).unwrap()
}

#[test]
fn test_config_param_32_34_36() {
    let prev_validators = get_validator_set();
    let cp32 = ConfigParam32 { prev_validators };
    write_read_and_assert(cp32);

    let cur_validators = get_validator_set();
    let cp34 = ConfigParam34 { cur_validators };
    write_read_and_assert(cp34);

    let next_validators = get_validator_set();
    let cp36 = ConfigParam36 { next_validators };
    write_read_and_assert(cp36);
}

fn get_workchain_desc() -> WorkchainDescr {
    let format = if rand::random::<u8>() > 128 {
        WorkchainFormat::Basic(WorkchainFormat1::with_params(123, 453454))
    } else {
        WorkchainFormat::Extended(WorkchainFormat0::with_params(64, 128, 64, 1).unwrap())
    };
    WorkchainDescr {
        enabled_since: 332,
        accept_msgs: true,
        active: false,
        flags: 0x345,
        max_split: 32,
        min_split: 2,
        version: 1,
        zerostate_file_hash: UInt256::rand(),
        zerostate_root_hash: UInt256::rand(),
        format,
        ..Default::default()
    }
}

fn get_workchain_desc_v2() -> WorkchainDescr {
    let format = if rand::random::<u8>() > 128 {
        WorkchainFormat::Basic(WorkchainFormat1::with_params(123, 453454))
    } else {
        WorkchainFormat::Extended(WorkchainFormat0::with_params(64, 128, 64, 1).unwrap())
    };
    WorkchainDescr {
        enabled_since: 332,
        accept_msgs: true,
        active: false,
        flags: 0x345,
        max_split: 32,
        min_split: 2,
        version: 1,
        zerostate_file_hash: UInt256::rand(),
        zerostate_root_hash: UInt256::rand(),
        format,
        split_merge_timings: Some(WcSplitMergeTimings::default()),
        persistent_state_split_depth: 2,
        ..Default::default()
    }
}

fn get_config_param11() -> ConfigParam11 {
    let normal_params = ConfigProposalSetup {
        min_tot_rounds: 1,
        max_tot_rounds: 2,
        min_wins: 3,
        max_losses: 4,
        min_store_sec: 5,
        max_store_sec: 6,
        bit_price: 7,
        cell_price: 8,
    };
    let critical_params = ConfigProposalSetup {
        min_tot_rounds: 10,
        max_tot_rounds: 20,
        min_wins: 30,
        max_losses: 40,
        min_store_sec: 50000,
        max_store_sec: 60000,
        bit_price: 70000,
        cell_price: 80000,
    };
    ConfigVotingSetup::new(&normal_params, &critical_params).unwrap()
}

fn get_config_param12() -> ConfigParam12 {
    let mut cp12 = ConfigParam12::default();

    for i in 0..rand::random::<u8>() as i32 + 2 {
        let wc = if i % 2 == 0 { get_workchain_desc() } else { get_workchain_desc_v2() };
        cp12.insert(i, &wc).unwrap();
    }
    cp12
}

#[test]
fn test_config_param_12() {
    let cp12 = get_config_param12();
    write_read_and_assert(cp12.clone());

    cp12.workchains.iterate(|_| -> Result<bool> { Ok(true) }).unwrap();
}

#[test]
fn test_config_params() {
    let mut cp = ConfigParams::default();

    let c0 = ConfigParamEnum::ConfigParam0(get_config_param0());
    cp.set_config(c0.clone()).unwrap();
    let c = cp.config(0).unwrap().unwrap();
    assert_eq!(c0, c);

    write_read_and_assert(cp.clone());

    let c1 = ConfigParamEnum::ConfigParam1(get_config_param1());
    cp.set_config(c1.clone()).unwrap();
    let c = cp.config(1).unwrap().unwrap();
    assert_eq!(c1, c);

    write_read_and_assert(cp.clone());

    let c2 =
        ConfigParamEnum::ConfigParam2(ConfigParam2 { minter_addr: AccountId::from([123; 32]) });
    cp.set_config(c2.clone()).unwrap();
    let c = cp.config(2).unwrap().unwrap();
    assert_eq!(c2, c);

    write_read_and_assert(cp.clone());

    let c3 = ConfigParamEnum::ConfigParam3(ConfigParam3 {
        fee_collector_addr: AccountId::from([133; 32]),
    });
    cp.set_config(c3.clone()).unwrap();
    let c = cp.config(3).unwrap().unwrap();
    assert_eq!(c3, c);

    write_read_and_assert(cp.clone());

    let dns_root_addr = AccountId::from([144; 32]);
    let c4 = ConfigParamEnum::ConfigParam4(ConfigParam4 { dns_root_addr });
    cp.set_config(c4.clone()).unwrap();
    let c = cp.config(4).unwrap().unwrap();
    assert_eq!(c4, c);

    let c5 = ConfigParamEnum::ConfigParam5(BurningConfig {
        blackhole_addr: Some(UInt256::rand().into()),
        fee_burn_num: 17,
        fee_burn_denom: 55,
    });
    cp.set_config(c5.clone()).unwrap();
    let c = cp.config(5).unwrap().unwrap();
    assert_eq!(c5, c);

    write_read_and_assert(cp.clone());

    let c6 = ConfigParamEnum::ConfigParam6(ConfigParam6 {
        mint_new_price: Coins::new(123),
        mint_add_price: Coins::new(1458347523),
    });
    cp.set_config(c6.clone()).unwrap();
    let c = cp.config(6).unwrap().unwrap();
    assert_eq!(c6, c);

    write_read_and_assert(cp.clone());

    let c7 = ConfigParamEnum::ConfigParam7(get_config_param7());
    cp.set_config(c7.clone()).unwrap();
    let c = cp.config(7).unwrap().unwrap();
    assert_eq!(c7, c);

    write_read_and_assert(cp.clone());

    let c8 = ConfigParamEnum::ConfigParam8(ConfigParam8 {
        global_version: GlobalVersion { version: 123, capabilities: 4567890 },
    });
    cp.set_config(c8.clone()).unwrap();
    let c = cp.config(8).unwrap().unwrap();
    assert_eq!(c8, c);

    write_read_and_assert(cp.clone());

    let c9 = ConfigParamEnum::ConfigParam9(get_config_param9());
    cp.set_config(c9.clone()).unwrap();
    let c = cp.config(9).unwrap().unwrap();
    assert_eq!(c9, c);

    write_read_and_assert(cp.clone());

    let c10 = ConfigParamEnum::ConfigParam10(get_config_param10());
    cp.set_config(c10.clone()).unwrap();
    let c = cp.config(10).unwrap().unwrap();
    assert_eq!(c10, c);

    write_read_and_assert(cp.clone());

    let c11 = ConfigParamEnum::ConfigParam11(get_config_param11());
    cp.set_config(c11.clone()).unwrap();
    let c = cp.config(11).unwrap().unwrap();
    assert_eq!(c11, c);

    write_read_and_assert(cp.clone());

    let c12 = ConfigParamEnum::ConfigParam12(get_config_param12());
    cp.set_config(c12.clone()).unwrap();
    let c = cp.config(12).unwrap().unwrap();
    assert_eq!(c12, c);

    write_read_and_assert(cp.clone());

    let c14 = ConfigParamEnum::ConfigParam14(get_config_param14());
    cp.set_config(c14.clone()).unwrap();
    let c = cp.config(14).unwrap().unwrap();
    assert_eq!(c14, c);

    write_read_and_assert(cp.clone());

    let c15 = ConfigParamEnum::ConfigParam15(get_config_param15());
    cp.set_config(c15.clone()).unwrap();
    let c = cp.config(15).unwrap().unwrap();
    assert_eq!(c15, c);

    write_read_and_assert(cp.clone());

    let c16 = ConfigParamEnum::ConfigParam16(get_config_param16());
    cp.set_config(c16.clone()).unwrap();
    let c = cp.config(16).unwrap().unwrap();
    assert_eq!(c16, c);

    write_read_and_assert(cp.clone());

    let c17 = ConfigParamEnum::ConfigParam17(get_config_param17());
    cp.set_config(c17.clone()).unwrap();
    let c = cp.config(17).unwrap().unwrap();
    assert_eq!(c17, c);

    write_read_and_assert(cp.clone());

    let c18 = ConfigParamEnum::ConfigParam18(get_config_param18());
    cp.set_config(c18.clone()).unwrap();
    let c = cp.config(18).unwrap().unwrap();
    assert_eq!(c18, c);

    write_read_and_assert(cp.clone());

    let c19 = ConfigParamEnum::ConfigParam19(765);
    cp.set_config(c19.clone()).unwrap();
    let c = cp.config(19).unwrap().unwrap();
    assert_eq!(c19, c);

    write_read_and_assert(cp.clone());

    let c20 = ConfigParamEnum::ConfigParam20(get_gas_limit_prices());
    cp.set_config(c20.clone()).unwrap();
    let c = cp.config(20).unwrap().unwrap();
    assert_eq!(c20, c);

    write_read_and_assert(cp.clone());

    let c21 = ConfigParamEnum::ConfigParam21(get_gas_limit_prices());
    cp.set_config(c21.clone()).unwrap();
    let c = cp.config(21).unwrap().unwrap();
    assert_eq!(c21, c);

    write_read_and_assert(cp.clone());

    let cp22 = get_block_limits(22, true);
    let c22 = ConfigParamEnum::ConfigParam22(cp22);
    cp.set_config(c22.clone()).unwrap();
    let c = cp.config(22).unwrap().unwrap();
    assert_eq!(c22, c);

    let cp23 = get_block_limits(23, false);
    let c23 = ConfigParamEnum::ConfigParam23(cp23);
    cp.set_config(c23.clone()).unwrap();
    let c = cp.config(23).unwrap().unwrap();
    assert_eq!(c23, c);

    write_read_and_assert(cp.clone());

    let c24 = ConfigParamEnum::ConfigParam24(get_msg_forward_prices());
    cp.set_config(c24.clone()).unwrap();
    let c = cp.config(24).unwrap().unwrap();
    assert_eq!(c24, c);

    write_read_and_assert(cp.clone());

    let c25 = ConfigParamEnum::ConfigParam25(get_msg_forward_prices());
    cp.set_config(c25.clone()).unwrap();
    let c = cp.config(25).unwrap().unwrap();
    assert_eq!(c25, c);

    write_read_and_assert(cp.clone());

    let c28 = ConfigParamEnum::ConfigParam28(get_cat_chain_config());
    cp.set_config(c28.clone()).unwrap();
    let c = cp.config(28).unwrap().unwrap();
    assert_eq!(c28, c);

    write_read_and_assert(cp.clone());

    let c29 = ConfigParamEnum::ConfigParam29(get_config_param29());
    cp.set_config(c29.clone()).unwrap();
    let c = cp.config(29).unwrap().unwrap();
    assert_eq!(c29, c);

    write_read_and_assert(cp.clone());

    let c31 = ConfigParamEnum::ConfigParam31(get_config_param31());
    cp.set_config(c31.clone()).unwrap();
    let c = cp.config(31).unwrap().unwrap();
    assert_eq!(c31, c);

    write_read_and_assert(cp.clone());

    assert!(cp
        .prev_validator_set()
        .expect("it should not fail, but gives empty list")
        .list()
        .is_empty());
    assert!(!cp.prev_validator_set_present().unwrap());

    let prev_validators = get_validator_set();
    let c32 = ConfigParamEnum::ConfigParam32(ConfigParam32 { prev_validators });
    cp.set_config(c32.clone()).unwrap();
    let c = cp.config(32).unwrap().unwrap();
    assert_eq!(c32, c);

    assert!(cp.prev_validator_set_present().unwrap());
    write_read_and_assert(cp.clone());

    let cur_validators = get_validator_set();
    let c34 = ConfigParamEnum::ConfigParam34(ConfigParam34 { cur_validators });
    cp.set_config(c34.clone()).unwrap();
    let c = cp.config(34).unwrap().unwrap();
    assert_eq!(c34, c);

    write_read_and_assert(cp.clone());

    assert!(cp
        .next_validator_set()
        .expect("it should not fail, but gives empty list")
        .list()
        .is_empty());
    assert!(!cp.next_validator_set_present().unwrap());

    let next_validators = get_validator_set();
    let c36 = ConfigParamEnum::ConfigParam36(ConfigParam36 { next_validators });
    cp.set_config(c36.clone()).unwrap();
    let c = cp.config(36).unwrap().unwrap();
    assert_eq!(c36, c);

    assert!(cp.next_validator_set_present().unwrap());

    write_read_and_assert(cp.clone());

    let cp39 = get_config_param_39();
    let c39 = ConfigParamEnum::ConfigParam39(cp39);
    cp.set_config(c39.clone()).unwrap();
    let c = cp.config(39).unwrap().unwrap();
    assert_eq!(c39, c);

    write_read_and_assert(cp.clone());

    let cp40 = get_config_param_40();
    let c40 = ConfigParamEnum::ConfigParam40(cp40);
    cp.set_config(c40.clone()).unwrap();
    let c = cp.config(40).unwrap().unwrap();
    assert_eq!(c40, c);

    write_read_and_assert(cp.clone());

    let c43 = SizeLimitsConfig::default();
    cp.set_config(ConfigParamEnum::ConfigParam43(c43.clone())).unwrap();
    let c = cp.config(43).unwrap().unwrap();
    assert_eq!(ConfigParamEnum::ConfigParam43(c43), c);

    write_read_and_assert(cp.clone());

    let c44 = get_suspended_address_list();
    cp.set_config(ConfigParamEnum::ConfigParam44(c44.clone())).unwrap();
    let c = cp.suspended_address_list().unwrap().unwrap();
    assert_eq!(c44, c);

    let c45 = get_precompiled_contracts_list();
    cp.set_config(ConfigParamEnum::ConfigParam45(c45.clone())).unwrap();
    let c = cp.precompiled_contracts_list().unwrap().unwrap();
    assert_eq!(c45, c);

    let oracle_bridge_params = OracleBridgeParams::default();

    cp.set_config(ConfigParamEnum::ConfigParam71(oracle_bridge_params.clone())).unwrap();
    let ConfigParamEnum::ConfigParam71(c) = cp.config(71).unwrap().unwrap() else {
        panic!("Expected ConfigParam71");
    };
    assert_eq!(oracle_bridge_params, c);

    cp.set_config(ConfigParamEnum::ConfigParam72(oracle_bridge_params.clone())).unwrap();
    let ConfigParamEnum::ConfigParam72(c) = cp.config(72).unwrap().unwrap() else {
        panic!("Expected ConfigParam72");
    };
    assert_eq!(oracle_bridge_params, c);

    cp.set_config(ConfigParamEnum::ConfigParam73(oracle_bridge_params.clone())).unwrap();
    let ConfigParamEnum::ConfigParam73(c) = cp.config(73).unwrap().unwrap() else {
        panic!("Expected ConfigParam73");
    };
    assert_eq!(oracle_bridge_params, c);

    let jetton_bridge_params = JettonBridgeParams::default();

    cp.set_config(ConfigParamEnum::ConfigParam79(jetton_bridge_params.clone())).unwrap();
    let ConfigParamEnum::ConfigParam79(c) = cp.config(79).unwrap().unwrap() else {
        panic!("Expected ConfigParam79");
    };
    assert_eq!(jetton_bridge_params, c);

    cp.set_config(ConfigParamEnum::ConfigParam81(jetton_bridge_params.clone())).unwrap();
    let ConfigParamEnum::ConfigParam81(c) = cp.config(81).unwrap().unwrap() else {
        panic!("Expected ConfigParam81");
    };
    assert_eq!(jetton_bridge_params, c);

    cp.set_config(ConfigParamEnum::ConfigParam82(jetton_bridge_params.clone())).unwrap();
    let ConfigParamEnum::ConfigParam82(c) = cp.config(82).unwrap().unwrap() else {
        panic!("Expected ConfigParam82");
    };
    assert_eq!(jetton_bridge_params, c);

    write_read_and_assert(cp.clone());
}

fn get_config_param_39() -> ConfigParam39 {
    let mut cp = ConfigParam39::default();

    let keypair = Ed25519KeyOption::<ZeroizingBytes>::generate().unwrap();
    let spk = SigPubKey::from_bytes(keypair.pub_key().unwrap()).unwrap();
    let cs = CryptoSignature::with_r_s(&[1; 32], &[2; 32]);
    let vtk = ValidatorTempKey::with_params(UInt256::from([3; 32]), spk, 100500, 1562663724);
    let vstk = ValidatorSignedTempKey::with_key_and_signature(vtk, cs);
    cp.insert(&UInt256::from([1; 32]), &vstk).unwrap();

    let keypair = Ed25519KeyOption::<ZeroizingBytes>::generate().unwrap();
    let spk = SigPubKey::from_bytes(keypair.pub_key().unwrap()).unwrap();
    let cs = CryptoSignature::with_r_s(&[6; 32], &[7; 32]);
    let vtk = ValidatorTempKey::with_params(UInt256::from([8; 32]), spk, 500100, 1562664724);
    let vstk = ValidatorSignedTempKey::with_key_and_signature(vtk, cs);
    cp.insert(&UInt256::from([2; 32]), &vstk).unwrap();

    cp
}

fn get_config_param_40() -> MisbehaviourPunishmentConfig {
    let mut rng = rand::thread_rng();
    MisbehaviourPunishmentConfig {
        default_flat_fine: Coins::new(rng.gen()),
        default_proportional_fine: rng.gen(),
        severity_flat_mult: rng.gen(),
        severity_proportional_mult: rng.gen(),
        unpunishable_interval: rng.gen(),
        long_interval: rng.gen(),
        long_flat_mult: rng.gen(),
        long_proportional_mult: rng.gen(),
        medium_interval: rng.gen(),
        medium_flat_mult: rng.gen(),
        medium_proportional_mult: rng.gen(),
    }
}

#[test]
fn test_config_param_39() {
    write_read_and_assert(get_config_param_39());
}

fn get_block_limits(some_val: u32, v2: bool) -> BlockLimits {
    BlockLimits::with_limits(
        ParamLimits::with_limits(some_val + 1, some_val + 2, some_val + 3).unwrap(),
        ParamLimits::with_limits(some_val + 4, some_val + 5, some_val + 6).unwrap(),
        ParamLimits::with_limits(some_val + 7, some_val + 8, some_val + 9).unwrap(),
        if v2 { Some(ParamLimits::with_limits(100000, 200000, 300000).unwrap()) } else { None },
        if v2 { Some(ImportedMsgQueueLimits::new(10080, 1717)) } else { None },
    )
}

#[test]
fn test_config_param_22_23() {
    let cp22: ConfigParam22 = get_block_limits(10000, true);
    write_read_and_assert(cp22);

    let cp23: ConfigParam23 = get_block_limits(777, false);
    write_read_and_assert(cp23);
}

#[test]
#[should_panic]
fn test_wrong_param_limits1() {
    let _ = ParamLimits::with_limits(10, 7, 30).unwrap();
}

#[test]
#[should_panic]
fn test_wrong_param_limits2() {
    let _ = ParamLimits::with_limits(10, 17, 11).unwrap();
}

#[test]
fn test_param_limits() {
    let underload = 100;
    let soft = 200;
    let hard = 300;
    let medium = (soft + hard) / 2;

    let l = ParamLimits::with_limits(underload, soft, hard).unwrap();

    assert_eq!(l.underload(), underload);
    assert_eq!(l.soft_limit(), soft);
    assert_eq!(l.hard_limit(), hard);
    assert_eq!(l.medium(), medium);

    assert_eq!(l.classify(hard), ParamLimitIndex::Hard);
    assert_eq!(l.classify(hard + 1), ParamLimitIndex::Hard);

    assert_eq!(l.classify(medium), ParamLimitIndex::Medium);
    assert_eq!(l.classify(hard - 1), ParamLimitIndex::Medium);

    assert_eq!(l.classify(soft), ParamLimitIndex::Soft);
    assert_eq!(l.classify(medium - 1), ParamLimitIndex::Soft);

    assert_eq!(l.classify(underload), ParamLimitIndex::Normal);
    assert_eq!(l.classify(soft - 1), ParamLimitIndex::Normal);

    assert_eq!(l.classify(underload - 1), ParamLimitIndex::Underload);

    // 0..200 is normal
    assert!(l.fits_normal(80, 50));
    assert!(!l.fits_normal(110, 50));
}

#[test]
fn test_block_limits() {
    let bl = BlockLimits::with_limits(
        ParamLimits::with_limits(100, 200, 300).unwrap(),
        ParamLimits::with_limits(1000, 2000, 3000).unwrap(),
        ParamLimits::with_limits(10000, 20000, 30000).unwrap(),
        None,
        None,
    );

    // 0..Underload
    assert!(bl.fits(ParamLimitIndex::Underload, 0, 0, 0));
    assert!(!bl.fits(ParamLimitIndex::Underload, 200, 0, 0));

    // 0..Soft
    assert!(bl.fits(ParamLimitIndex::Normal, 0, 0, 0));
    assert!(bl.fits(ParamLimitIndex::Normal, 150, 1500, 15000));
    assert!(bl.fits(ParamLimitIndex::Normal, 150, 1999, 0));
    assert!(!bl.fits(ParamLimitIndex::Normal, 250, 1999, 0));
    assert!(!bl.fits(ParamLimitIndex::Normal, 250, 2999, 40000));
    assert!(!bl.fits(ParamLimitIndex::Normal, 200, 0, 0));

    // 0..Medium
    assert!(bl.fits(ParamLimitIndex::Soft, 0, 0, 0));
    assert!(bl.fits(ParamLimitIndex::Soft, 249, 2499, 2499));
    assert!(!bl.fits(ParamLimitIndex::Soft, 250, 1999, 0));

    // 0..Hard
    assert!(bl.fits(ParamLimitIndex::Medium, 0, 0, 0));
    assert!(bl.fits(ParamLimitIndex::Medium, 299, 2999, 2999));
    assert!(!bl.fits(ParamLimitIndex::Medium, 350, 1999, 0));

    // 0..∞
    assert!(bl.fits(ParamLimitIndex::Hard, 0, 0, 0));
    assert!(bl.fits(ParamLimitIndex::Hard, 249, 2499, 2499));
    assert!(bl.fits(ParamLimitIndex::Hard, 100000, 100000, 100000));
}

fn get_config_param7() -> ConfigParam7 {
    let mut ecc = ExtraCurrencyCollection::default();
    for _ in 1..100 {
        ecc.set(
            &rand::random::<u32>(),
            &VarUInteger32::from_two_u128(
                rand::random::<u128>() & 0x00ffffff_ffffffff_ffffffff_ffffffff, // VarUInteger32 stores 31 bytes NOT 32!!!
                rand::random::<u128>(),
            )
            .unwrap(),
        )
        .unwrap();
    }
    ConfigParam7 { to_mint: ecc }
}

fn get_config_param9() -> ConfigParam9 {
    let mut mp = MandatoryParams::default();
    for _ in 1..100 {
        mp.set(&rand::random::<u32>(), &EmptyValue).unwrap();
    }
    ConfigParam9 { mandatory_params: mp }
}

fn get_config_param10() -> ConfigParam10 {
    let mut cp = MandatoryParams::default();
    for _ in 1..100 {
        cp.set(&rand::random::<u32>(), &EmptyValue).unwrap();
    }
    ConfigParam10 { critical_params: cp }
}

fn get_config_param14() -> ConfigParam14 {
    ConfigParam14 {
        block_create_fees: BlockCreateFees {
            masterchain_block_fee: Coins::new(1458347523),
            basechain_block_fee: Coins::new(145800000000003),
        },
    }
}

fn get_config_param15() -> ConfigParam15 {
    ConfigParam15 {
        validators_elected_for: rand::random::<u32>(),
        elections_start_before: rand::random::<u32>(),
        elections_end_before: rand::random::<u32>(),
        stake_held_for: rand::random::<u32>(),
    }
}

fn get_config_param29() -> ConsensusConfig {
    ConsensusConfig {
        new_catchain_ids: true,
        round_candidates: rand::random::<u8>() as u32 | 1,
        next_candidate_delay_ms: rand::random::<u32>(),
        consensus_timeout_ms: rand::random::<u32>(),
        fast_attempts: rand::random::<u32>(),
        attempt_duration: rand::random::<u32>(),
        catchain_max_deps: rand::random::<u32>(),
        max_block_bytes: rand::random::<u32>(),
        max_collated_bytes: rand::random::<u32>(),
        catchain_max_blocks_coeff: 0,
        proto_version: 0,
    }
}

fn get_suspended_address_list() -> SuspendedAddressList {
    let mut sa = SuspendedAddressList::default();
    let mut addr = [0; 32];
    for _ in 1..100 {
        (0..32).for_each(|i| addr[i] = rand::random::<u8>());
        sa.add_suspended_address(rand::random::<i32>() % 2, addr.into()).unwrap();
    }
    sa
}

fn get_precompiled_contracts_list() -> PrecompiledContractsList {
    let mut pl = PrecompiledContractsList::default();
    for _ in 1..100 {
        pl.add(&UInt256::rand(), rand::random::<u64>()).unwrap();
    }
    pl
}

#[test]
fn test_suspended_addresses() {
    write_read_and_assert(get_suspended_address_list());
}

#[test]
fn test_real_ton_config_params() {
    let bytes = std::fs::read("src/tests/data/config.boc").unwrap();
    let cell = read_single_root_boc(bytes).unwrap();
    let config1 = ConfigParams::with_address_and_params(AccountId::from([1; 32]), Some(cell));
    dump_config(&config1.config_params);
    assert!(!config1.valid_config_data(false, None).unwrap()); // fake config address
    assert!(config1.valid_config_data(true, None).unwrap()); // but other are ok
    let mut config2 = config1.clone();
    assert!(!config1.important_config_parameters_changed(&config2, true).unwrap());
    assert!(!config1.important_config_parameters_changed(&config2, false).unwrap());

    if let Some(ConfigParamEnum::ConfigParam0(param)) = config1.config(0).unwrap() {
        config2.config_addr = param.config_addr;
    }
    assert!(config2.valid_config_data(false, None).unwrap()); // real adress
    assert!(config2.valid_config_data(true, None).unwrap());

    assert!(!config1.important_config_parameters_changed(&config2, true).unwrap());
    assert!(!config1.important_config_parameters_changed(&config2, false).unwrap());

    if let Ok(Some(ConfigParamEnum::ConfigParam9(param))) = config1.config(9) {
        println!("Mandatory params indeces {:?}", param.mandatory_params.export_keys::<i32>());
    }
    if let Ok(Some(ConfigParamEnum::ConfigParam10(param))) = config1.config(10) {
        println!("Critical params indeces {:?}", param.critical_params.export_keys::<i32>());
    }
    //  remove mandatory parameter - make config not valid
    let key = SliceData::load_builder(14u32.write_to_new_cell().unwrap()).unwrap();
    config2.config_params.remove(key).unwrap();
    assert!(!config2.valid_config_data(true, None).unwrap());
}

#[test]
fn test_calc_storage_fees_max() {
    let sp = StoragePrices {
        utime_since: 0,
        bit_price_ps: 1,
        cell_price_ps: 500,
        mc_bit_price_ps: 1000,
        mc_cell_price_ps: 500000,
    };
    let max_int64 = i64::MAX as u64;
    assert_eq!(max_int64, 0x7FFF_FFFF_FFFF_FFFF);
    let fee = sp.calc_storage_fee_part(max_int64, max_int64, max_int64, true);
    assert_eq!(fee, "42620366456847542539545850799900483125749000".parse().unwrap());
}

#[test]
fn test_accelerated_consensus_config() {
    let config = AcceleratedConsensusConfig {
        enabled: true,
        failed_collation_retry_timeout_ms: 1000,
        skip_rounds_count_for_collator_rotation: 5,
        max_precollated_blocks: 11,
    };
    let cell = config.write_to_new_cell().unwrap().into_cell().unwrap();
    let config2 = AcceleratedConsensusConfig::construct_from_cell(cell).unwrap();
    assert_eq!(config, config2);
}

#[test]
fn test_simplex_config() {
    let config = SimplexConfig {
        slots_per_leader_window: 4,
        noncritical_params: NoncriticalParams {
            target_rate_ms: 300,
            first_block_timeout_ms: 1000,
            max_leader_window_desync: 100,
            ..Default::default()
        },
        ..Default::default()
    };
    let cell = config.write_to_new_cell().unwrap().into_cell().unwrap();
    let config2 = SimplexConfig::construct_from_cell(cell).unwrap();
    assert_eq!(config, config2);
}

#[test]
fn test_simplex_config_with_quic() {
    let config = SimplexConfig {
        use_quic: true,
        slots_per_leader_window: 4,
        noncritical_params: NoncriticalParams {
            target_rate_ms: 300,
            first_block_timeout_ms: 1000,
            max_leader_window_desync: 100,
            ..Default::default()
        },
        ..Default::default()
    };
    let cell = config.write_to_new_cell().unwrap().into_cell().unwrap();
    let config2 = SimplexConfig::construct_from_cell(cell).unwrap();
    assert_eq!(config, config2);
    assert!(config2.use_quic);
}

#[test]
fn test_new_consensus_config_all_both() {
    let mc_config = SimplexConfig {
        slots_per_leader_window: 4,
        noncritical_params: NoncriticalParams {
            target_rate_ms: 300,
            first_block_timeout_ms: 1000,
            max_leader_window_desync: 100,
            ..Default::default()
        },
        ..Default::default()
    };
    let shard_config = SimplexConfig {
        slots_per_leader_window: 8,
        noncritical_params: NoncriticalParams {
            target_rate_ms: 200,
            first_block_timeout_ms: 500,
            max_leader_window_desync: 50,
            ..Default::default()
        },
        ..Default::default()
    };
    let config = NewConsensusConfigAll { mc: Some(mc_config), shard: Some(shard_config) };
    let cell = config.write_to_new_cell().unwrap().into_cell().unwrap();
    let config2 = NewConsensusConfigAll::construct_from_cell(cell).unwrap();
    assert_eq!(config, config2);
}

#[test]
fn test_new_consensus_config_all_shard_only() {
    let shard_config = SimplexConfig {
        slots_per_leader_window: 8,
        noncritical_params: NoncriticalParams {
            target_rate_ms: 200,
            first_block_timeout_ms: 500,
            max_leader_window_desync: 50,
            ..Default::default()
        },
        ..Default::default()
    };
    let config = NewConsensusConfigAll { mc: None, shard: Some(shard_config) };
    let cell = config.write_to_new_cell().unwrap().into_cell().unwrap();
    let config2 = NewConsensusConfigAll::construct_from_cell(cell).unwrap();
    assert_eq!(config, config2);
}

#[test]
fn test_new_consensus_config_all_mc_only() {
    let mc_config = SimplexConfig {
        slots_per_leader_window: 4,
        noncritical_params: NoncriticalParams {
            target_rate_ms: 300,
            first_block_timeout_ms: 1000,
            max_leader_window_desync: 100,
            ..Default::default()
        },
        ..Default::default()
    };
    let config = NewConsensusConfigAll { mc: Some(mc_config), shard: None };
    let cell = config.write_to_new_cell().unwrap().into_cell().unwrap();
    let config2 = NewConsensusConfigAll::construct_from_cell(cell).unwrap();
    assert_eq!(config, config2);
}

#[test]
fn test_new_consensus_config_all_empty() {
    let config = NewConsensusConfigAll { mc: None, shard: None };
    let cell = config.write_to_new_cell().unwrap().into_cell().unwrap();
    let config2 = NewConsensusConfigAll::construct_from_cell(cell).unwrap();
    assert_eq!(config, config2);
}

// ===================== simplex_config v2 serialization tests =====================

#[test]
fn test_simplex_config_v2_round_trip() {
    let config = SimplexConfig {
        use_quic: true,
        slots_per_leader_window: 8,
        noncritical_params: NoncriticalParams {
            target_rate_ms: 2400,
            first_block_timeout_ms: 1000,
            max_leader_window_desync: 250,
            candidate_resolve_rate_limit: 10,
            min_block_interval_ms: 333,
            no_empty_blocks_on_error_timeout_ms: 22_000,
            ..Default::default()
        },
        ..Default::default()
    };
    let cell = config.write_to_new_cell().unwrap().into_cell().unwrap();
    let config2 = SimplexConfig::construct_from_cell(cell).unwrap();
    assert_eq!(config, config2);
}

#[test]
fn test_simplex_config_default_round_trip() {
    let config = SimplexConfig::default();
    let cell = config.write_to_new_cell().unwrap().into_cell().unwrap();
    let config2 = SimplexConfig::construct_from_cell(cell).unwrap();
    assert_eq!(config, config2);
}

#[test]
fn test_simplex_config_custom_noncritical_params() {
    let config = SimplexConfig {
        use_quic: true,
        slots_per_leader_window: 6,
        noncritical_params: NoncriticalParams {
            target_rate_ms: 500,
            first_block_timeout_ms: 2000,
            max_leader_window_desync: 100,
            first_block_timeout_multiplier_bits: (1.5f32).to_bits(),
            bad_signature_ban_duration_ms: 10_000,
            ..Default::default()
        },
        ..Default::default()
    };
    let cell = config.write_to_new_cell().unwrap().into_cell().unwrap();
    let config2 = SimplexConfig::construct_from_cell(cell).unwrap();
    assert_eq!(config, config2);
}

// ===================== v1 deserialization backward-compat tests =====================

/// Build a raw simplex_config#21 cell by hand (the legacy on-chain format).
fn build_v1_cell(use_quic: bool, target_rate_ms: u32, slots: u32, fbt_ms: u32, mld: u32) -> Cell {
    let mut b = BuilderData::new();
    b.append_u8(0x21).unwrap();
    b.append_u8(if use_quic { 1 } else { 0 }).unwrap();
    target_rate_ms.write_to(&mut b).unwrap();
    slots.write_to(&mut b).unwrap();
    fbt_ms.write_to(&mut b).unwrap();
    mld.write_to(&mut b).unwrap();
    b.into_cell().unwrap()
}

#[test]
fn test_deserialize_v1_cell() {
    let cell = build_v1_cell(false, 300, 4, 1000, 100);
    let config = SimplexConfig::construct_from_cell(cell).unwrap();
    assert!(!config.use_quic);
    assert_eq!(config.slots_per_leader_window, 4);
    assert_eq!(config.noncritical_params.target_rate_ms, 300);
    assert_eq!(config.noncritical_params.first_block_timeout_ms, 1000);
    assert_eq!(config.noncritical_params.max_leader_window_desync, 100);
    let d = NoncriticalParams::default();
    assert_eq!(
        config.noncritical_params.first_block_timeout_multiplier_bits,
        d.first_block_timeout_multiplier_bits
    );
}

#[test]
fn test_deserialize_v1_cell_with_quic() {
    let cell = build_v1_cell(true, 300, 4, 1000, 100);
    let config = SimplexConfig::construct_from_cell(cell).unwrap();
    assert!(config.use_quic);
}

#[test]
fn test_new_consensus_config_all_with_v2() {
    let config = SimplexConfig {
        use_quic: true,
        slots_per_leader_window: 8,
        noncritical_params: NoncriticalParams {
            target_rate_ms: 2400,
            first_block_timeout_ms: 1000,
            max_leader_window_desync: 250,
            ..Default::default()
        },
        ..Default::default()
    };

    let all = NewConsensusConfigAll { mc: Some(config.clone()), shard: Some(config.clone()) };
    let cell = all.write_to_new_cell().unwrap().into_cell().unwrap();
    let parsed = NewConsensusConfigAll::construct_from_cell(cell).unwrap();
    assert_eq!(parsed.mc.unwrap(), config);
    assert_eq!(parsed.shard.unwrap(), config);
}

#[test]
fn test_simplex_config_enable_observers_round_trip() {
    let config = SimplexConfig {
        enable_observers: true,
        use_quic: true,
        slots_per_leader_window: 4,
        ..Default::default()
    };
    let cell = config.write_to_new_cell().unwrap().into_cell().unwrap();
    let config2 = SimplexConfig::construct_from_cell(cell).unwrap();
    assert_eq!(config, config2);
    assert!(config2.enable_observers);
    assert!(config2.use_quic);
}

/// Verifies the v2 flag-byte wire layout:
/// MSB->LSB = 6 zero flag bits, enable_observers (bit 1), use_quic (bit 0).
#[test]
fn test_simplex_config_v2_flag_byte_layout() {
    fn flag_byte(enable_observers: bool, use_quic: bool) -> u8 {
        let cfg = SimplexConfig { enable_observers, use_quic, ..Default::default() };
        let cell = cfg.write_to_new_cell().unwrap().into_cell().unwrap();
        let mut s = SliceData::load_cell(cell).unwrap();
        let tag = s.get_next_byte().unwrap();
        assert_eq!(tag, 0x22);
        s.get_next_byte().unwrap()
    }
    assert_eq!(flag_byte(false, false), 0x00);
    assert_eq!(flag_byte(false, true), 0x01);
    assert_eq!(flag_byte(true, false), 0x02);
    assert_eq!(flag_byte(true, true), 0x03);
}

/// v1 (`simplex_config#21`) had no enable_observers; we keep lenient decode
/// to preserve archive replay
#[test]
fn test_deserialize_v1_cell_enable_observers_is_false() {
    let cell = build_v1_cell(true, 300, 4, 1000, 100);
    let config = SimplexConfig::construct_from_cell(cell).unwrap();
    assert!(!config.enable_observers);
    assert!(config.use_quic);
}

#[test]
fn test_new_consensus_config_all_mixed_v1_mc_v2_shard() {
    let mc_cell = build_v1_cell(false, 300, 4, 1000, 100);
    let shard_config = SimplexConfig {
        slots_per_leader_window: 6,
        noncritical_params: NoncriticalParams {
            target_rate_ms: 500,
            first_block_timeout_ms: 2000,
            ..Default::default()
        },
        ..Default::default()
    };
    let shard_cell = shard_config.write_to_new_cell().unwrap().into_cell().unwrap();

    let mut builder = BuilderData::new();
    builder.append_u8(0x10).unwrap();
    builder.append_bit_one().unwrap();
    builder.checked_append_reference(mc_cell).unwrap();
    builder.append_bit_one().unwrap();
    builder.checked_append_reference(shard_cell).unwrap();
    let cell = builder.into_cell().unwrap();

    let parsed = NewConsensusConfigAll::construct_from_cell(cell).unwrap();
    let mc = parsed.mc.unwrap();
    assert_eq!(mc.noncritical_params.target_rate_ms, 300);

    let shard = parsed.shard.unwrap();
    assert_eq!(shard.noncritical_params.target_rate_ms, 500);
    assert_eq!(shard.noncritical_params.first_block_timeout_ms, 2000);
    assert_eq!(
        shard.noncritical_params.max_leader_window_desync,
        NoncriticalParams::default().max_leader_window_desync
    );
}
