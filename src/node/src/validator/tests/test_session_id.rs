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
use ton_block::{signature::SigPubKey, validators::ValidatorDescr};

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
