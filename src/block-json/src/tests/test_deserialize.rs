/*
 * Copyright (C) 2019-2023 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;
use crate::{serialize_config, serialize_config_param, SerializationMode};
use ton_block::{
    BuilderData, ConfigParam3, ConfigParam32, ConfigParam33, ConfigParam35, ConfigParam36,
    ConfigParam37, ConfigParam39, ConfigParam4, ConfigParam6, ConfigVotingSetup, IBitstring,
    Number16, SigPubKey, VarUInteger32,
};

include!("./test_common.rs");

#[test]
fn test_parse_zerostate() {
    let ethalon = std::fs::read_to_string("src/tests/data/zerostate-ethalon.json").unwrap();
    let map = serde_json::from_str::<Map<String, Value>>(&ethalon).unwrap();
    let state = parse_state(&map).unwrap();
    let json = crate::debug_state_full(state).unwrap();
    assert_json_eq(&json, &ethalon, "zerostate");
}

fn check_err<T: std::fmt::Debug>(result: Result<T>, text: &str) {
    let len = text.len();
    assert_eq!(&result.expect_err("must generate error").to_string()[0..len], text)
}

#[test]
fn test_parse_errors() {
    let json = serde_json::json!({
        "obj": {
            "a1": "12345678901234567890",
            "a2": "qwe",
            "a3": 123.4567890
        },
        "array": [
            {
                "a1": "123"
            },
            {
                "a1": 123
            }
        ],
        "str": "qwe",
        "int": "-100",
        "uint": "-100",
    });

    let map = PathMap::new(json.as_object().unwrap());
    check_err(map.get_obj("unknown"), "root must have the field `unknown`");
    check_err(map.get_vec("obj"), "root/obj must be the vector");
    let obj = map.get_obj("obj").unwrap();
    check_err(obj.get_obj("a1"), "root/obj/a1 must be the object");
    check_err(obj.get_num("a1"), "root/obj/a1 must be the integer or a string with the integer");
    check_err(obj.get_num("a2"), "root/obj/a2 must be the integer or a string with the integer");
    check_err(obj.get_num("a3"), "root/obj/a3 must be the integer or a string with the integer");
}

fn get_config_param0() -> ConfigParam0 {
    let mut c = ConfigParam0::new();
    c.config_addr = [1; 32].into();
    c
}

fn get_config_param1() -> ConfigParam1 {
    let mut c = ConfigParam1::new();
    c.elector_addr = [1; 32].into();
    c
}

fn get_config_param7() -> ConfigParam7 {
    let mut ecc = ExtraCurrencyCollection::default();
    for i in 1..100 {
        ecc.set(&(i as u32), &VarUInteger32::from_two_u128(i * 100, i * 205).unwrap()).unwrap();
    }
    ConfigParam7 { to_mint: ecc }
}

fn get_config_param16() -> ConfigParam16 {
    let mut c = ConfigParam16::new();
    c.max_validators = Number16::new(23424).unwrap();
    c.max_main_validators = Number16::new(35553).unwrap();
    c.min_validators = Number16::new(11).unwrap();
    c
}

fn get_config_param17() -> ConfigParam17 {
    let mut c = ConfigParam17::new();
    c.min_stake = Grams::zero();
    c.max_stake = Grams::one();
    c.max_stake_factor = 12121;
    c
}

fn get_storage_prices() -> StoragePrices {
    let mut st = StoragePrices::new();
    st.bit_price_ps = 10;
    st.cell_price_ps = 20;
    st.mc_bit_price_ps = 30;
    st.mc_cell_price_ps = 40;
    st.utime_since = 50;
    st
}

fn get_config_param18() -> ConfigParam18 {
    let mut cp18 = ConfigParam18::default();
    for _ in 0..10 {
        cp18.insert(&get_storage_prices()).unwrap();
    }
    cp18
}

fn get_config_param19() -> GlobalId {
    786
}

fn get_gas_limit_prices() -> GasLimitsPrices {
    let mut glp = GasLimitsPrices {
        gas_price: 10,
        gas_limit: 20,
        gas_credit: 30,
        block_gas_limit: 40,
        freeze_due_limit: 50,
        delete_due_limit: 60,
        special_gas_limit: 70,
        flat_gas_limit: 80,
        flat_gas_price: 90,
        max_gas_threshold: 0,
    };
    glp.max_gas_threshold = glp.calc_max_gas_threshold(glp.gas_limit);
    glp
}

fn get_msg_forward_prices() -> MsgForwardPrices {
    MsgForwardPrices {
        lump_price: 10,
        bit_price: 20,
        cell_price: 30,
        ihr_price_factor: 40,
        first_frac: 50,
        next_frac: 60,
    }
}

fn get_cat_chain_config() -> CatchainConfig {
    let mut cc = CatchainConfig::new();
    cc.shuffle_mc_validators = true;
    cc.isolate_mc_validators = false;
    cc.mc_catchain_lifetime = 10;
    cc.shard_catchain_lifetime = 20;
    cc.shard_validators_lifetime = 30;
    cc.shard_validators_num = 40;
    cc
}

fn get_config_param31() -> ConfigParam31 {
    let mut cp31 = ConfigParam31::new();
    for _ in 1..10 {
        cp31.add_address(UInt256::rand().into());
    }
    cp31
}

fn get_workchain_desc() -> WorkchainDescr {
    let mut wc = WorkchainDescr::new();
    wc.enabled_since = 332;
    wc.accept_msgs = true;
    wc.active = false;
    wc.flags = 0x345;
    wc.version = 1;
    wc.zerostate_file_hash = UInt256::rand();
    wc.zerostate_root_hash = UInt256::rand();

    wc.format = WorkchainFormat::Basic(WorkchainFormat1::with_params(123, 453454));
    wc
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
    let mut cp12 = ConfigParam12::new();

    for i in 0..10_i32 {
        let wc = get_workchain_desc();
        cp12.insert(i, &wc).unwrap();
    }
    cp12
}

fn get_config_param9() -> ConfigParam9 {
    let mut mp = MandatoryParams::default();
    for i in 1..100 {
        mp.add_key(&i).unwrap();
    }
    ConfigParam9 { mandatory_params: mp }
}

fn get_config_param10() -> ConfigParam10 {
    let mut cp = MandatoryParams::default();
    for i in 1..100 {
        cp.add_key(&i).unwrap();
    }
    ConfigParam10 { critical_params: cp }
}

fn get_config_param14() -> ConfigParam14 {
    ConfigParam14 {
        block_create_fees: BlockCreateFees {
            masterchain_block_fee: Grams::from(1458347523u64),
            basechain_block_fee: Grams::from(145800000000003u64),
        },
    }
}

fn get_config_param15() -> ConfigParam15 {
    ConfigParam15 {
        validators_elected_for: 10,
        elections_start_before: 20,
        elections_end_before: 30,
        stake_held_for: 40,
    }
}

fn get_config_param29() -> ConsensusConfig {
    ConsensusConfig {
        new_catchain_ids: true,
        round_candidates: 10_u32 | 1,
        next_candidate_delay_ms: 20,
        consensus_timeout_ms: 30,
        fast_attempts: 40,
        attempt_duration: 50,
        catchain_max_deps: 60,
        max_block_bytes: 70,
        max_collated_bytes: 80,
        catchain_max_blocks_coeff: 0,
        proto_version: 0,
    }
}

fn get_config_param44() -> SuspendedAddressList {
    let mut cfg = SuspendedAddressList::default();
    cfg.set_suspended_until(1742976363);
    cfg.add_suspended_address(-1, [0; 32].into()).unwrap();
    cfg.add_suspended_address(0, [0xFF; 32].into()).unwrap();
    cfg
}

fn get_config_param45() -> PrecompiledContractsList {
    let mut cfg = PrecompiledContractsList::default();
    cfg.add(&[0; 32].into(), 157).unwrap();
    cfg.add(&[0xFF; 32].into(), 300).unwrap();
    cfg
}

fn get_config_param63() -> AcceleratedConsensusConfig {
    AcceleratedConsensusConfig {
        enabled: false,
        failed_collation_retry_timeout_ms: 2000,
        skip_rounds_count_for_collator_rotation: 7,
        max_precollated_blocks: 11,
    }
}

fn get_config_param30() -> NewConsensusConfigAll {
    NewConsensusConfigAll {
        mc: Some(SimplexConfig {
            target_rate_ms: 300,
            slots_per_leader_window: 4,
            first_block_timeout_ms: 1000,
            max_leader_window_desync: 100,
        }),
        shard: Some(SimplexConfig {
            target_rate_ms: 200,
            slots_per_leader_window: 8,
            first_block_timeout_ms: 500,
            max_leader_window_desync: 50,
        }),
    }
}

fn get_block_limits(some_val: u32, extra_limits: bool) -> BlockLimits {
    let (c_limit, mq_limit) = if extra_limits {
        (
            Some(ParamLimits::with_limits(some_val + 10, some_val + 11, some_val + 12).unwrap()),
            Some(ImportedMsgQueueLimits::new(some_val + 100, some_val + 1001)),
        )
    } else {
        (None, None)
    };
    BlockLimits::with_limits(
        ParamLimits::with_limits(some_val + 1, some_val + 2, some_val + 3).unwrap(),
        ParamLimits::with_limits(some_val + 4, some_val + 5, some_val + 6).unwrap(),
        ParamLimits::with_limits(some_val + 7, some_val + 8, some_val + 9).unwrap(),
        c_limit,
        mq_limit,
    )
}

fn get_config_param_39() -> ConfigParam39 {
    let mut cp = ConfigParam39::default();
    let vstk = ValidatorSignedTempKey::construct_from_base64(
        "te6ccgEBAgEAlAABgwRQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEB\
         AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgKAEAmgMDAwMDAwMDAwMD\
         AwMDAwMDAwMDAwMDAwMDAwMDAwMDA46BJ4rNcGUgnAwuWyQw+RaHbPRm6ub11x\
         lYhzyXOQiirgIdZgABiJRdJFss",
    )
    .unwrap();
    cp.insert(&UInt256::from([1; 32]), &vstk).unwrap();
    let vstk = ValidatorSignedTempKey::construct_from_base64(
        "te6ccgEBAgEAlAABgwRQYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYG\
          BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBweAEAmgMICAgICAgICAgI\
          CAgICAgICAgICAgICAgICAgICAgICI6BJ4obim1cCeb6yST1ojiep2h7raTYuN\
          UkJEbwlPIjN4qOLAAHoYRdJF8U",
    )
    .unwrap();
    cp.insert(&UInt256::from([1; 32]), &vstk).unwrap();
    cp
}

fn get_validator_set() -> ValidatorSet {
    let mut list = vec![];
    let key = SigPubKey::from_bytes(
        &base64_decode("39MLqLIVrzLqPCHCFpbn1/jILSbfNMtnr/7zOkKE1Ds=").unwrap(),
    )
    .unwrap();
    let vd = ValidatorDescr::with_params(key, 4, None);
    list.push(vd);
    let key = SigPubKey::from_bytes(
        &base64_decode("BIYYOFHTgVDIFzVLhuSZw2ne1J3zuv75zwYhAXb0+iY=").unwrap(),
    )
    .unwrap();
    let vd = ValidatorDescr::with_params(key, 5, None);
    list.push(vd);
    ValidatorSet::new(0, 100, 1, list).unwrap()
}

fn prepare_config_params() -> ConfigParams {
    let mut cp = ConfigParams::default();

    let c0 = ConfigParamEnum::ConfigParam0(get_config_param0());
    cp.set_config(c0).unwrap();

    let c1 = ConfigParamEnum::ConfigParam1(get_config_param1());
    cp.set_config(c1).unwrap();

    let c2 = ConfigParamEnum::ConfigParam2(ConfigParam2 { minter_addr: [123; 32].into() });
    cp.set_config(c2).unwrap();

    let c3 = ConfigParamEnum::ConfigParam3(ConfigParam3 { fee_collector_addr: [133; 32].into() });
    cp.set_config(c3).unwrap();

    let c4 = ConfigParamEnum::ConfigParam4(ConfigParam4 { dns_root_addr: [144; 32].into() });
    cp.set_config(c4).unwrap();

    let c5 = ConfigParamEnum::ConfigParam5(BurningConfig {
        blackhole_addr: Some([200; 32].into()),
        fee_burn_num: 17,
        fee_burn_denom: 55,
    });
    cp.set_config(c5).unwrap();

    let c6 = ConfigParamEnum::ConfigParam6(ConfigParam6 {
        mint_new_price: Grams::from(123u64),
        mint_add_price: Grams::from(1458347523u64),
    });
    cp.set_config(c6).unwrap();

    let c7 = ConfigParamEnum::ConfigParam7(get_config_param7());
    cp.set_config(c7).unwrap();

    let c8 = ConfigParamEnum::ConfigParam8(ConfigParam8 {
        global_version: GlobalVersion { version: 123, capabilities: 4567890 },
    });
    cp.set_config(c8).unwrap();

    let c9 = ConfigParamEnum::ConfigParam9(get_config_param9());
    cp.set_config(c9).unwrap();

    let c10 = ConfigParamEnum::ConfigParam10(get_config_param10());
    cp.set_config(c10).unwrap();

    let c11 = ConfigParamEnum::ConfigParam11(get_config_param11());
    cp.set_config(c11).unwrap();

    let c12 = ConfigParamEnum::ConfigParam12(get_config_param12());
    cp.set_config(c12).unwrap();

    let mut builder = BuilderData::new();
    builder.append_u32(100).unwrap();
    let c13 = ConfigParamEnum::ConfigParam13(ConfigParam13 { cell: builder.into_cell().unwrap() });
    cp.set_config(c13).unwrap();

    let c14 = ConfigParamEnum::ConfigParam14(get_config_param14());
    cp.set_config(c14).unwrap();

    let c15 = ConfigParamEnum::ConfigParam15(get_config_param15());
    cp.set_config(c15).unwrap();

    let c16 = ConfigParamEnum::ConfigParam16(get_config_param16());
    cp.set_config(c16).unwrap();

    let c17 = ConfigParamEnum::ConfigParam17(get_config_param17());
    cp.set_config(c17).unwrap();

    let c18 = ConfigParamEnum::ConfigParam18(get_config_param18());
    cp.set_config(c18).unwrap();

    let c19 = ConfigParamEnum::ConfigParam19(get_config_param19());
    cp.set_config(c19).unwrap();

    let c20 = ConfigParamEnum::ConfigParam20(get_gas_limit_prices());
    cp.set_config(c20).unwrap();

    let c21 = ConfigParamEnum::ConfigParam21(get_gas_limit_prices());
    cp.set_config(c21).unwrap();

    let cp22 = get_block_limits(22, false);
    let c22 = ConfigParamEnum::ConfigParam22(cp22);
    cp.set_config(c22).unwrap();

    let cp23 = get_block_limits(23, true);
    let c23 = ConfigParamEnum::ConfigParam23(cp23);
    cp.set_config(c23).unwrap();

    let c24 = ConfigParamEnum::ConfigParam24(get_msg_forward_prices());
    cp.set_config(c24).unwrap();

    let c25 = ConfigParamEnum::ConfigParam25(get_msg_forward_prices());
    cp.set_config(c25).unwrap();

    let c28 = ConfigParamEnum::ConfigParam28(get_cat_chain_config());
    cp.set_config(c28).unwrap();

    let c29 = ConfigParamEnum::ConfigParam29(get_config_param29());
    cp.set_config(c29).unwrap();

    let c31 = ConfigParamEnum::ConfigParam31(get_config_param31());
    cp.set_config(c31).unwrap();

    let cp32 = ConfigParam32::with_validator_set(get_validator_set());
    cp.set_config(ConfigParamEnum::ConfigParam32(cp32)).unwrap();

    let cp33 = ConfigParam33::with_validator_set(get_validator_set());
    cp.set_config(ConfigParamEnum::ConfigParam33(cp33)).unwrap();

    let cp34 = ConfigParam34::with_validator_set(get_validator_set());
    cp.set_config(ConfigParamEnum::ConfigParam34(cp34)).unwrap();

    let cp35 = ConfigParam35::with_validator_set(get_validator_set());
    cp.set_config(ConfigParamEnum::ConfigParam35(cp35)).unwrap();

    let cp36 = ConfigParam36::with_validator_set(get_validator_set());
    cp.set_config(ConfigParamEnum::ConfigParam36(cp36)).unwrap();

    let cp37 = ConfigParam37::with_validator_set(get_validator_set());
    cp.set_config(ConfigParamEnum::ConfigParam37(cp37)).unwrap();

    let cp39 = get_config_param_39();
    cp.set_config(ConfigParamEnum::ConfigParam39(cp39)).unwrap();

    let c40 = Default::default();
    cp.set_config(ConfigParamEnum::ConfigParam40(c40)).unwrap();

    let c43 = Default::default();
    cp.set_config(ConfigParamEnum::ConfigParam43(c43)).unwrap();

    let c44 = get_config_param44();
    cp.set_config(ConfigParamEnum::ConfigParam44(c44)).unwrap();

    let c45 = get_config_param45();
    cp.set_config(ConfigParamEnum::ConfigParam45(c45)).unwrap();

    let c45 = get_config_param45();
    cp.set_config(ConfigParamEnum::ConfigParam45(c45)).unwrap();

    let c30 = get_config_param30();
    cp.set_config(ConfigParamEnum::ConfigParam30(c30)).unwrap();

    let c63 = get_config_param63();
    cp.set_config(ConfigParamEnum::ConfigParam63(c63)).unwrap();

    let oracle_bridge_params = OracleBridgeParams::default();

    cp.set_config(ConfigParamEnum::ConfigParam71(oracle_bridge_params.clone())).unwrap();
    cp.set_config(ConfigParamEnum::ConfigParam72(oracle_bridge_params.clone())).unwrap();
    cp.set_config(ConfigParamEnum::ConfigParam73(oracle_bridge_params)).unwrap();

    let jetton_bridge_params = JettonBridgeParams::default();

    cp.set_config(ConfigParamEnum::ConfigParam79(jetton_bridge_params.clone())).unwrap();
    cp.set_config(ConfigParamEnum::ConfigParam81(jetton_bridge_params.clone())).unwrap();
    cp.set_config(ConfigParamEnum::ConfigParam82(jetton_bridge_params)).unwrap();

    cp
}

#[test]
fn test_config_params() {
    let cp = prepare_config_params();

    let check_params = |old: &ConfigParams, new: &ConfigParams| {
        for i in 0..45 {
            if old.config_present(i).unwrap() {
                let old_conf = old.config(i).unwrap().unwrap();
                let new_conf = new.config(i).unwrap().unwrap();
                assert_eq!(old_conf, new_conf);
            } else {
                assert!(!new.config_present(i).unwrap());
            }
        }
    };

    let mut json = serde_json::Map::<String, Value>::new();
    serialize_config(&mut json, &cp, SerializationMode::QServer).unwrap();
    let parsed_config = parse_config(json.get("config").unwrap().as_object().unwrap()).unwrap();
    check_params(&cp, &parsed_config);

    let mut json = serde_json::Map::<String, Value>::new();
    serialize_config(&mut json, &cp, SerializationMode::Debug).unwrap();
    let parsed_config = parse_config(json.get("config").unwrap().as_object().unwrap()).unwrap();
    check_params(&cp, &parsed_config);
}

#[test]
fn test_parse_config_params() {
    let cp = prepare_config_params();
    for index in 0..45 {
        if let Ok(param) = serialize_config_param(&cp, index) {
            println!("{}: {}", index, param);
            let config = serde_json::from_str(&param).unwrap();
            let cp_new = parse_config_with_mandatory_params(&config, &[index]).unwrap();
            assert_eq!(cp.config(index).unwrap(), cp_new.config(index).unwrap());
        }
    }
}

#[test]
fn test_parse_block_proof() {
    let boc = include_bytes!("data/block_proof");
    let ethalon_proof = ton_block::BlockProof::construct_from_bytes(boc).unwrap();
    let json = serde_json::from_str(include_str!("data/proof-ethalon.json")).unwrap();

    let parsed_proof = parse_block_proof(&json, ethalon_proof.proof_for.file_hash.clone()).unwrap();
    assert_eq!(ethalon_proof, parsed_proof);
    assert_eq!(boc.as_slice(), &parsed_proof.write_to_bytes().unwrap());
}

/// Test backward compatibility: parse old JSON format without signature_type field
/// This ensures migration from old codebase to new codebase works seamlessly
#[test]
fn test_parse_block_proof_legacy_format_without_signature_type() {
    use ton_block::BlockSignaturesVariant;

    let boc = include_bytes!("data/block_proof");
    let ethalon_proof = ton_block::BlockProof::construct_from_bytes(boc).unwrap();

    // Parse legacy JSON without signature_type field
    let json = serde_json::from_str(include_str!("data/proof-ethalon-legacy.json")).unwrap();
    let parsed_proof = parse_block_proof(&json, ethalon_proof.proof_for.file_hash.clone()).unwrap();

    // Verify it parses as Ordinary variant (the default)
    match parsed_proof.signatures.as_ref().unwrap() {
        BlockSignaturesVariant::Ordinary(sig) => {
            // Verify the signatures match the original
            assert_eq!(
                sig.validator_info.validator_list_hash_short,
                ethalon_proof
                    .signatures
                    .as_ref()
                    .unwrap()
                    .validator_info()
                    .validator_list_hash_short
            );
            assert_eq!(
                sig.validator_info.catchain_seqno,
                ethalon_proof.signatures.as_ref().unwrap().validator_info().catchain_seqno
            );
            assert_eq!(
                sig.pure_signatures.weight(),
                ethalon_proof.signatures.as_ref().unwrap().pure_signatures().weight()
            );
        }
        BlockSignaturesVariant::Simplex(_) => {
            panic!("Legacy format should parse as Ordinary, not Simplex");
        }
    }

    // Verify proof_for matches
    assert_eq!(parsed_proof.proof_for, ethalon_proof.proof_for);
}

/// Test that explicit signature_type=ordinary also works
#[test]
fn test_parse_block_proof_explicit_ordinary_signature_type() {
    use ton_block::BlockSignaturesVariant;

    let boc = include_bytes!("data/block_proof");
    let ethalon_proof = ton_block::BlockProof::construct_from_bytes(boc).unwrap();

    // Parse JSON with explicit signature_type: "ordinary"
    let json = serde_json::from_str(include_str!("data/proof-ethalon.json")).unwrap();
    let parsed_proof = parse_block_proof(&json, ethalon_proof.proof_for.file_hash.clone()).unwrap();

    // Verify it parses as Ordinary variant
    match parsed_proof.signatures.as_ref().unwrap() {
        BlockSignaturesVariant::Ordinary(_) => {
            // Expected
        }
        BlockSignaturesVariant::Simplex(_) => {
            panic!("Explicit ordinary should parse as Ordinary, not Simplex");
        }
    }

    // Binary roundtrip should match original
    assert_eq!(boc.as_slice(), &parsed_proof.write_to_bytes().unwrap());
}

#[test]
fn test_parse_block_proof_with_simplex_signatures() {
    use ton_block::{
        BlockProof, BlockSignaturesPure, BlockSignaturesSimplex, BlockSignaturesVariant,
        CryptoSignature, CryptoSignaturePair, UInt256, ValidatorBaseInfo,
    };

    // Load a real block proof from test data
    let boc = include_bytes!("data/block_proof");
    let original_proof = ton_block::BlockProof::construct_from_bytes(boc).unwrap();

    // Create simplex signatures
    let mut pure_signatures = BlockSignaturesPure::new();
    pure_signatures.set_weight(12345678);
    pure_signatures.add_sigpair(CryptoSignaturePair {
        node_id_short: UInt256::from([0x11; 32]),
        sign: CryptoSignature::with_r_s(&[0x22; 32], &[0x33; 32]),
    });

    let validator_info = ValidatorBaseInfo::with_params(99999, 88888);
    let session_id = UInt256::from([0xBB; 32]);
    let slot = 123u32;
    let candidate_data = vec![0x12, 0x34, 0x56, 0x78];
    let data = BlockSignaturesSimplex::bytes_to_cell_tree(&candidate_data).unwrap();

    let simplex_sigs = BlockSignaturesSimplex::new_finalize(
        validator_info,
        pure_signatures,
        session_id.clone(),
        slot,
        data,
    );

    // Create proof with simplex signatures
    let proof = BlockProof::with_params(
        original_proof.proof_for.clone(),
        original_proof.root.clone(),
        Some(BlockSignaturesVariant::Simplex(simplex_sigs)),
    );

    // Serialize to JSON
    let json_map = crate::db_serialize_block_proof("_id", &proof).unwrap();

    // Parse back from JSON
    let parsed_proof = parse_block_proof(&json_map, proof.proof_for.file_hash.clone()).unwrap();

    // Verify the signatures match
    match parsed_proof.signatures.as_ref().unwrap() {
        BlockSignaturesVariant::Simplex(s) => {
            assert_eq!(s.session_id, session_id);
            assert_eq!(s.slot, slot);
            assert!(s.is_final);
            assert_eq!(s.validator_info.validator_list_hash_short, 99999);
            assert_eq!(s.validator_info.catchain_seqno, 88888);
            assert_eq!(s.pure_signatures.weight(), 12345678);
        }
        BlockSignaturesVariant::Ordinary(_) => {
            panic!("Expected Simplex signatures, got Ordinary");
        }
    }

    // Verify proof_for matches
    assert_eq!(parsed_proof.proof_for, proof.proof_for);
}
