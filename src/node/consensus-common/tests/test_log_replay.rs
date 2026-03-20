/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use consensus_common::{ConsensusCommonFactory, LogReplayOptions};
use std::path::Path;
use storage::db::rocksdb::destroy_rocks_db;

const DB_PATH: &str = "../../target/test";

#[tokio::test]
async fn log_replay_multiple_players() {
    const DB_NAME: &str = "catchains_log_replay_multiple_players";

    let db_path = Path::new(DB_PATH).join(DB_NAME).display().to_string();
    let mut replay_opts = LogReplayOptions::with_db(db_path);
    replay_opts.log_file_name = "tests/log_header.log".to_string();
    let log_players = ConsensusCommonFactory::create_log_players(&replay_opts);

    for log_player in log_players {
        println!(
            "Session {} with {} nodes and local_id {} has been detected",
            log_player.get_session_id().to_hex_string(),
            log_player.get_nodes().len(),
            log_player.get_local_id()
        );
    }

    drop(replay_opts);
    destroy_rocks_db(DB_PATH, DB_NAME).await.unwrap()
}
