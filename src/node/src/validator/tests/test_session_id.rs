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
use super::*;
use crate::validator::{log_parser::LogParser, validator_utils::GeneralSessionInfo};
use std::{
    fs,
    io::{self, BufRead},
    sync::Arc,
    time::Duration,
};
use ton_block::{signature::SigPubKey, validators::ValidatorDescr, Ed25519KeyOption};

fn parse_shard_ident(parser: &LogParser, name: &str) -> ShardIdent {
    ShardIdent::with_tagged_prefix(
        parser.parse_field_fromstr::<i32>(&format!("{}.workchain", name)),
        parser.parse_field_fromstr::<u64>(&format!("{}.shard", name)),
    )
    .unwrap()
}

#[allow(dead_code)]
fn parse_duration(parser: &LogParser, field: &str) -> Duration {
    let duration = parser.parse_field_fromstr::<f64>(field);
    let secs = duration.floor();
    let nanosecs = duration - secs;
    if nanosecs != 0.0 {
        unimplemented!("Fractional duration");
    }
    Duration::from_secs(secs as u64)
}

fn parse_validator_descr(parser: &LogParser, name: &str) -> ValidatorDescr {
    ValidatorDescr::with_params(
        SigPubKey::from_bytes(&parser.parse_slice(&format!("{}.key", name))).unwrap(),
        parser.parse_field_fromstr::<u64>(&format!("{}.weight", name)),
        Some(UInt256::from_slice(parser.parse_slice(&format!("{}.addr", name)).as_slice())),
    )
}

struct ValidatorParams {
    general_session_info: Arc<GeneralSessionInfo>,
    val_set: ValidatorSet,
}

impl ValidatorParams {
    fn parse(parser: &LogParser) -> Self {
        let mut validators = Vec::new();
        for num in 0..parser.get_field_count("val_set") {
            validators.push(parse_validator_descr(parser, &format!("val_set.{}", num)));
        }

        let catchain_seqno = parser.parse_field_fromstr::<u32>("catchain_seqno");
        ValidatorParams {
            general_session_info: Arc::new(GeneralSessionInfo {
                catchain_seqno,
                opts_hash: UInt256::from_slice(parser.parse_slice("opts_hash").as_slice()),
                shard: parse_shard_ident(parser, "shard"),
                key_seqno: parser.parse_field_fromstr::<u32>("key_seqno"),
                max_vertical_seqno: 0,
            }),
            val_set: ValidatorSet::with_cc_seqno(0, 0, 0, catchain_seqno, validators).unwrap(),
        }
    }
}

fn do_test_get_validator_set_id(contents: &str) {
    let parser = LogParser::new(contents);
    /*
        let opts = validator_session::SessionOptions {
            catchain_idle_timeout: parse_duration(&parser, "catchain_idle_timeout"),
            catchain_max_deps: parser.parse_field_fromstr::<u32>("catchain_max_deps"),
            catchain_skip_processed_blocks: false,
            round_candidates: parser.parse_field_fromstr::<u32>("round_candidates"),
            next_candidate_delay: Duration::from_secs(2), //parse_duration(&parser, "next_candidate_delay"),
            round_attempt_duration: Duration::from_secs(parser.parse_field_fromstr::<u64>("round_attempt_duration")),
            max_round_attempts: parser.parse_field_fromstr::<u32>("max_round_attempts"),
            max_block_size: parser.parse_field_fromstr::<u32>("max_block_size"),
            max_collated_data_size: parser.parse_field_fromstr::<u32>("max_collated_data_size"),
            new_catchain_ids: parser.parse_field_fromstr::<u32>("new_catchain_ids") > 0
        };
    */
    let p = ValidatorParams::parse(&parser);

    let serialized =
        get_session_id_serialize(p.general_session_info.clone(), p.val_set.list(), true);
    let computed_id = get_session_id(p.general_session_info.clone(), p.val_set.list(), true, false);
    let actual_id = UInt256::from_slice(&parser.parse_slice("group_id"));

    println!("Serialized: {}", hex::encode(&serialized));
    println!("Actual group-id: {}", actual_id);
    println!("Computed group-id: {}", computed_id);

    assert_eq!(actual_id, computed_id);
}

#[test]
fn test_session_id_normal() {
    let file = fs::File::open("src/validator/tests/static/test_session_id_normal.log").unwrap();
    for line in io::BufReader::new(file).lines() {
        let line_unwrapped = line.unwrap();
        println!("Contents: {}", line_unwrapped);
        do_test_get_validator_set_id(&line_unwrapped);
    }
}

fn do_test_catchain_unsafe_rotate(s: &str) {
    let parser = LogParser::new(s);
    let p = ValidatorParams::parse(&parser);
    let prev_block = 1; //parser.parse_field_fromstr::<u32>("prev_block");
    let rotation_id = parser.parse_field_fromstr::<u32>("rotation_id");

    let mut config: ValidatorManagerConfig = ValidatorManagerConfig::default();
    config
        .unsafe_catchain_rotates
        .insert(p.general_session_info.catchain_seqno, (prev_block, rotation_id));

    //let session_id = get_session_id(&p.shard, &p.val_set, &p.opts_hash, p.key_seqno, true, 0);
    let session_id = get_session_id(p.general_session_info.clone(), p.val_set.list(), true, false);
    let unsafe_serialized = compute_session_unsafe_serialized(&session_id, rotation_id);
    let actual_serialized = parser.parse_slice("unsafe_serialized");

    let unsafe_id = get_session_unsafe_id(
        p.general_session_info.clone(),
        p.val_set.list(),
        true,
        true,
        Some(prev_block),
        &config,
        false,
    );
    let real_unsafe_id = UInt256::from_slice(&parser.parse_slice("unsafe_id"));

    println!("Actual unsafe-id: {:x}", real_unsafe_id);
    println!("Computed unsafe-id: {:x}", unsafe_id);

    println!("Actual unsafe-serialized: {}", hex::encode(actual_serialized));
    println!("Computed unsafe-serialized: {}", hex::encode(unsafe_serialized));

    assert_eq!(unsafe_id, real_unsafe_id);
}

#[test]
fn test_session_id_unsafe() {
    let file = fs::File::open("src/validator/tests/static/test_session_id_unsafe.log").unwrap();
    for line in io::BufReader::new(file).lines() {
        let line_unwrapped = line.unwrap();
        println!("Contents: {}", line_unwrapped);
        do_test_catchain_unsafe_rotate(&line_unwrapped);
    }
}

#[test]
fn test_session_id_unsafe_v2() {
    use crate::validator::validator_manager::get_validator_session_options_hash;
    use ton_block::base64_encode;
    {
        let opts = validator_session::SessionOptions {
            catchain_idle_timeout: Duration::from_secs(16),
            catchain_max_deps: 4,
            catchain_skip_processed_blocks: true,
            round_candidates: 3,
            next_candidate_delay: Duration::from_secs(2),
            proto_version: 4,
            catchain_max_serialized_block_size: 16384,
            catchain_block_hash_covers_data: true,
            catchain_max_block_height_coeff: 2500000,
            catchain_disable_db: false,
            catchain_receiver_max_neighbours_count: 5,
            catchain_receiver_neighbours_sync_min_period: Duration::from_millis(100),
            catchain_receiver_neighbours_sync_max_period: Duration::from_millis(200),
            catchain_receiver_max_sources_sync_attempts: 3,
            catchain_receiver_neighbours_rotate_min_period: Duration::from_secs(60),
            catchain_receiver_neighbours_rotate_max_period: Duration::from_secs(120),
            round_attempt_duration: Duration::from_secs(8),
            max_round_attempts: 3,
            max_block_size: 2097152,
            max_collated_data_size: 2097152,
            new_catchain_ids: true,
            skip_single_node_session_validations: false,
            ..Default::default()
        };

        let (hash, _serialized_data) = get_validator_session_options_hash(opts, 9407195);
        //println!("!!! Hash for {:?} is : {}", opts, base64_encode(hash));

        assert_eq!(base64_encode(hash), "zGHYA323cMOmXestPYStZs5hVCDfOY2mdQm2l9zF4Bo=");
    }
}

#[test]
fn test_cxx_interop_session_options_hash_ignores_accelerated_fields() {
    let base_opts = CatchainSessionOptions {
        proto_version: 4,
        round_candidates: 3,
        max_round_attempts: 4,
        max_block_size: 1024,
        max_collated_data_size: 2048,
        new_catchain_ids: true,
        ..Default::default()
    };
    let (base_hash, base_serialized) = get_validator_session_options_hash(base_opts.clone(), 100);

    let mut accelerated_opts = base_opts.clone();
    accelerated_opts.accelerated_consensus_enabled = true;
    accelerated_opts.accelerated_consensus_collation_retry_timeout = Duration::from_millis(777);
    accelerated_opts.accelerated_consensus_skip_rounds_count_for_collator_rotation = 9;
    accelerated_opts.accelerated_consensus_max_precollated_blocks = 17;

    let (accelerated_hash, _) = get_validator_session_options_hash(accelerated_opts.clone(), 100);
    let (interop_hash, interop_serialized) =
        get_cxx_interop_session_options_hash(&accelerated_opts, 100);

    assert_eq!(
        accelerated_hash, base_hash,
        "validator-session config hashing must stay C++-compatible even if runtime accelerated fields differ"
    );
    assert_eq!(interop_hash, base_hash, "interop hash must stay aligned with C++");
    assert_eq!(interop_serialized, base_serialized, "interop serialization must match C++ options");
}

fn make_test_consensus_config() -> ConsensusConfig {
    ConsensusConfig {
        new_catchain_ids: true,
        round_candidates: 3,
        next_candidate_delay_ms: 2000,
        consensus_timeout_ms: 16000,
        fast_attempts: 4,
        attempt_duration: 8,
        catchain_max_deps: 4,
        max_block_bytes: 1024,
        max_collated_bytes: 2048,
        proto_version: 4,
        catchain_max_blocks_coeff: 2500000,
    }
}

#[cfg(not(feature = "xp25"))]
#[test]
fn test_session_id_hashes_stay_shared_without_xp25() {
    let consensus_config = make_test_consensus_config();
    let catchain_config = CatchainConfig::default();
    let mc_options = CatchainSessionOptions {
        max_round_attempts: 5,
        max_block_size: 1024,
        max_collated_data_size: 2048,
        new_catchain_ids: true,
        proto_version: 4,
        ..Default::default()
    };
    let shard_options = CatchainSessionOptions {
        max_round_attempts: 6,
        max_block_size: 1024,
        max_collated_data_size: 2048,
        new_catchain_ids: true,
        proto_version: 4,
        ..Default::default()
    };

    let ((mc_hash, _), (shard_hash, _)) = get_session_id_hashes(
        &consensus_config,
        &catchain_config,
        &mc_options,
        &shard_options,
        100,
    );

    assert_eq!(mc_hash, shard_hash, "non-xp25 must keep one shared C++-compatible opts_hash");
}

#[cfg(feature = "xp25")]
#[test]
fn test_session_id_hashes_can_differ_with_xp25() {
    let consensus_config = make_test_consensus_config();
    let catchain_config = CatchainConfig::default();
    let mc_options = CatchainSessionOptions {
        max_round_attempts: 5,
        max_block_size: 1024,
        max_collated_data_size: 2048,
        new_catchain_ids: true,
        proto_version: 4,
        ..Default::default()
    };
    let shard_options = CatchainSessionOptions {
        max_round_attempts: 6,
        max_block_size: 1024,
        max_collated_data_size: 2048,
        new_catchain_ids: true,
        proto_version: 4,
        ..Default::default()
    };

    let ((mc_hash, _), (shard_hash, _)) = get_session_id_hashes(
        &consensus_config,
        &catchain_config,
        &mc_options,
        &shard_options,
        100,
    );

    assert_ne!(mc_hash, shard_hash, "xp25 must allow MC/shard opts_hash to diverge");
}

// ---------------------------------------------------------------------------
// ValidatorListStatus and validator-manager helper tests
// ---------------------------------------------------------------------------

fn make_test_key() -> PublicKey {
    Ed25519KeyOption::generate().unwrap()
}

fn make_validator_descr_from_key(key: &PublicKey) -> ValidatorDescr {
    ValidatorDescr::with_params(SigPubKey::from_bytes(key.pub_key().unwrap()).unwrap(), 1, None)
}

#[test]
fn test_validator_list_status_get_local_key_for_list() {
    let mut status = ValidatorListStatus::default();
    let key_a = make_test_key();
    let key_b = make_test_key();
    let list_curr = UInt256::from_slice(&[1u8; 32]);
    let list_next = UInt256::from_slice(&[2u8; 32]);

    status.add_list(list_curr.clone(), vec![key_a.clone()], true);
    status.add_list(list_next.clone(), vec![key_b.clone()], true);
    status.curr = Some(list_curr.clone());
    status.next = Some(list_next.clone());

    // get_local_keys returns only the curr list's keys
    let local = status.get_local_keys().unwrap();
    assert_eq!(local[0].id(), key_a.id());

    // get_local_keys_for_list returns the keys for the specified list
    let local_curr = status.get_local_keys_for_list(&list_curr).unwrap();
    assert_eq!(local_curr[0].id(), key_a.id());

    let local_next = status.get_local_keys_for_list(&list_next).unwrap();
    assert_eq!(local_next[0].id(), key_b.id());

    // unknown list returns None
    let unknown = UInt256::from_slice(&[3u8; 32]);
    assert!(status.get_local_keys_for_list(&unknown).is_none());
}

#[test]
fn test_validator_list_status_get_local_key_curr_none() {
    let mut status = ValidatorListStatus::default();
    let key = make_test_key();
    let list_next = UInt256::from_slice(&[2u8; 32]);

    status.add_list(list_next.clone(), vec![key.clone()], true);
    status.next = Some(list_next.clone());
    // curr is None

    // get_local_keys returns None when curr is None
    assert!(status.get_local_keys().is_none());

    // but get_local_keys_for_list still finds the next list key
    let found = status.get_local_keys_for_list(&list_next).unwrap();
    assert_eq!(found[0].id(), key.id());
}

#[test]
fn test_validator_list_status_actual_or_coming() {
    let mut status = ValidatorListStatus::default();
    let key = make_test_key();
    let list_curr = UInt256::from_slice(&[1u8; 32]);
    let list_next = UInt256::from_slice(&[2u8; 32]);
    let list_old = UInt256::from_slice(&[3u8; 32]);

    status.add_list(list_curr.clone(), vec![key.clone()], true);
    status.add_list(list_next.clone(), vec![key.clone()], true);
    status.curr = Some(list_curr.clone());
    status.next = Some(list_next.clone());

    assert!(status.actual_or_coming(&list_curr));
    assert!(status.actual_or_coming(&list_next));
    assert!(!status.actual_or_coming(&list_old));
}

#[test]
fn test_validator_list_status_network_readiness() {
    let mut status = ValidatorListStatus::default();
    let key = make_test_key();
    let list_id = UInt256::from_slice(&[9u8; 32]);

    status.add_list(list_id.clone(), vec![key.clone()], false);
    assert!(!status.is_list_network_ready(&list_id));
    assert_eq!(status.get_local_keys_for_list(&list_id).unwrap()[0].id(), key.id());

    status.add_list(list_id.clone(), vec![key], true);
    assert!(status.is_list_network_ready(&list_id));
}

#[test]
fn test_validator_list_status_ready_current_list_requires_network_readiness() {
    let mut status = ValidatorListStatus::default();
    let key = make_test_key();
    let list_id = UInt256::from_slice(&[7u8; 32]);

    status.add_list(list_id.clone(), vec![key.clone()], false);
    status.curr = Some(list_id.clone());
    assert!(status.get_ready_current_list().is_none());

    status.add_list(list_id.clone(), vec![key], true);
    assert_eq!(status.get_ready_current_list(), Some(&list_id));
}

#[test]
fn test_validator_list_status_ready_current_list_ignores_next_only_membership() {
    let mut status = ValidatorListStatus::default();
    let key = make_test_key();
    let next_list = UInt256::from_slice(&[8u8; 32]);

    status.add_list(next_list.clone(), vec![key], true);
    status.next = Some(next_list);

    assert!(status.get_ready_current_list().is_none());
}

#[test]
fn test_validator_list_status_next_only_ready_list_remains_usable_for_future_sessions() {
    let mut status = ValidatorListStatus::default();
    let key = make_test_key();
    let next_list = UInt256::from_slice(&[10u8; 32]);

    status.add_list(next_list.clone(), vec![key], true);
    status.next = Some(next_list.clone());

    assert!(status.get_ready_current_list().is_none());
    assert!(status.is_list_network_ready(&next_list));
}

#[test]
fn test_find_local_validator_key_uses_local_key_order_per_subset() {
    let key_a = make_test_key();
    let key_b = make_test_key();
    let validators = vec![make_validator_descr_from_key(&key_b)];
    let local_keys = vec![key_a, key_b.clone()];

    let selected = find_local_validator_key(&validators, Some(local_keys.as_slice()))
        .expect("second local key should match the subset");
    assert_eq!(selected.id(), key_b.id());
}

#[test]
fn test_session_id_new_catchain_ids_true_succeeds() {
    let session_info = Arc::new(GeneralSessionInfo {
        shard: ShardIdent::masterchain(),
        opts_hash: UInt256::default(),
        catchain_seqno: 1,
        key_seqno: 0,
        max_vertical_seqno: 0,
    });
    let key = make_test_key();
    let val = make_validator_descr_from_key(&key);

    let result = get_session_id_serialize(session_info, &[val], true);
    assert!(!result.is_empty());
}

#[test]
#[should_panic(expected = "Old catchain IDs format")]
fn test_session_id_new_catchain_ids_false_panics() {
    let session_info = Arc::new(GeneralSessionInfo {
        shard: ShardIdent::masterchain(),
        opts_hash: UInt256::default(),
        catchain_seqno: 1,
        key_seqno: 0,
        max_vertical_seqno: 0,
    });
    let key = make_test_key();
    let val = make_validator_descr_from_key(&key);

    // This should panic with the assert message
    let _ = get_session_id_serialize(session_info, &[val], false);
}

#[test]
fn test_session_id_with_accelerated_consensus() {
    let session_info = Arc::new(GeneralSessionInfo {
        shard: ShardIdent::masterchain(),
        opts_hash: UInt256::default(),
        catchain_seqno: 1,
        key_seqno: 0,
        max_vertical_seqno: 0,
    });
    let key = make_test_key();
    let val = make_validator_descr_from_key(&key);

    let id_without = get_session_id(session_info.clone(), &[val.clone()], true, false);
    let id_with = get_session_id(session_info, &[val], true, true);

    // Accelerated consensus tag changes the session ID
    assert_ne!(id_without, id_with);
}

#[test]
fn test_find_local_validator_key_matches_subset() {
    let key = make_test_key();
    let validator = make_validator_descr_from_key(&key);
    let local_keys = vec![key.clone()];

    let found = find_local_validator_key(&[validator], Some(local_keys.as_slice()))
        .expect("local validator key should match the subset");

    assert_eq!(found.id(), key.id());
}

#[test]
fn test_find_local_validator_key_returns_none_when_key_not_in_subset() {
    let key = make_test_key();
    let other_key = make_test_key();
    let validator = make_validator_descr_from_key(&other_key);
    let local_keys = vec![key];

    assert!(find_local_validator_key(&[validator], Some(local_keys.as_slice())).is_none());
}

#[test]
fn test_should_skip_session_for_unsafe_rotation_matches_cpp_policy() {
    let masterchain = ShardIdent::masterchain();
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap();

    assert!(!should_skip_session_for_unsafe_rotation(false, &masterchain));
    assert!(!should_skip_session_for_unsafe_rotation(false, &shard));
    assert!(!should_skip_session_for_unsafe_rotation(true, &masterchain));
    assert!(should_skip_session_for_unsafe_rotation(true, &shard));
}

#[test]
fn test_unsafe_rotation_block_seqno_uses_last_masterchain_block_only() {
    let masterchain = ShardIdent::masterchain();
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap();
    let last_masterchain_block = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        777,
        UInt256::default(),
        UInt256::default(),
    );

    assert_eq!(unsafe_rotation_block_seqno(&masterchain, &last_masterchain_block), Some(777));
    assert_eq!(unsafe_rotation_block_seqno(&shard, &last_masterchain_block), None);
}

#[test]
fn test_get_session_unsafe_id_skips_patch_when_flag_false() {
    let session_info = Arc::new(GeneralSessionInfo {
        shard: ShardIdent::masterchain(),
        opts_hash: UInt256::default(),
        catchain_seqno: 42,
        key_seqno: 0,
        max_vertical_seqno: 0,
    });
    let key = make_test_key();
    let val = make_validator_descr_from_key(&key);

    let mut config = ValidatorManagerConfig::default();
    config.unsafe_catchain_rotates.insert(42, (1, 99));

    let plain_id = get_session_id(session_info.clone(), &[val.clone()], true, false);

    let id_with_flag_false = get_session_unsafe_id(
        session_info.clone(),
        &[val.clone()],
        true,
        false,
        Some(100),
        &config,
        false,
    );
    assert_eq!(id_with_flag_false, plain_id, "flag=false must return the plain session ID");

    let id_with_flag_true =
        get_session_unsafe_id(session_info, &[val], true, true, Some(100), &config, false);
    assert_ne!(id_with_flag_true, plain_id, "flag=true must apply the unsafe rotation patch");
}
