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
use crate::{
    config::TonNodeConfig,
    engine::{run, Engine, EngineFlags, Stopper},
    test_helper::{configure_ip, get_config, init_test},
};
use std::{fs::remove_dir_all, sync::Arc, time::Duration};
use ton_block::Result;

fn start_node(
    rt: &tokio::runtime::Runtime,
    stopper: Arc<Stopper>,
    config: TonNodeConfig,
) -> Result<(Arc<Engine>, tokio::task::JoinHandle<()>)> {
    let validator_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(8 * 1024 * 1024)
        .build()
        .expect("Can't create Validator tokio runtime")
        .handle()
        .clone();
    let liteserver_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(8 * 1024 * 1024)
        .build()
        .expect("Can't create LiteServer tokio runtime")
        .handle()
        .clone();
    let flags =
        EngineFlags { initial_sync_disabled: true, force_check_db: false, truncate_db: None };
    rt.block_on(run(config, None, validator_rt, liteserver_rt, flags, stopper, None))
}

#[ignore]
#[test]
fn test_node_restart() {
    const CONFIG_FROM_INITBLOCK: &str = "default_config_mainet_initblock_test.json";
    const CONFIG_FROM_ZEROSTATE: &str = "default_config_mainet_test.json";
    const DB_PATH: &str = "../target/node_restart";

    for step in 1..=4 {
        remove_dir_all(DB_PATH).ok();

        // Steps:
        //   1 - isolated node (no replies from network expected), start from zerostate
        //   2 - isolated node (no replies from network expected), start from initblock
        //   3 - connected node, start from zerostate
        //   4 - connected node, start from initblock
        let (config, ip) = match step {
            1 => (CONFIG_FROM_ZEROSTATE, "127.0.0.1:5191".to_string()),
            2 => (CONFIG_FROM_INITBLOCK, "127.0.0.1:5191".to_string()),
            3 => (CONFIG_FROM_ZEROSTATE, configure_ip("0.0.0.0:1", 4190)),
            _ => (CONFIG_FROM_INITBLOCK, configure_ip("0.0.0.0:1", 4190)),
        };

        for i in 1..=2 {
            println!("Step {} Iteration #{}", step, i);
            let rt = init_test();
            let stopper = Arc::new(Stopper::new());
            let mut config = rt.block_on(get_config(&ip, None, None, config)).unwrap();
            config.set_internal_db_path(DB_PATH.to_string());
            let stopper_clone = stopper.clone();
            rt.spawn(async move {
                tokio::time::sleep(Duration::from_millis(20000)).await;
                stopper_clone.set_stop();
                println!("Node stop signal sent");
            });
            let stopper_clone = stopper.clone();
            match start_node(&rt, stopper, config) {
                Err(e) => {
                    if !stopper_clone.check_stop() {
                        panic!("Can't start node engine: {}", e)
                    } else {
                        println!("Node stopped (in start phase).");
                    }
                }
                Ok((engine, join_handle)) => rt.block_on(async move {
                    join_handle.await.ok();
                    println!("Stopping node...");
                    engine.wait_stop().await;
                    println!("Node stopped (in run phase).");
                }),
            }
        }
    }
}
