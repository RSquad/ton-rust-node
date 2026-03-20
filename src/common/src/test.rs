/*
 * Copyright 2018-2022 TON DEV SOLUTIONS LTD.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the Apache License, Version 2.0.
 * See the common/LICENSE file in this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
include!("./info.rs");
include!("./log.rs");

pub fn init_test_log() {
    use std::{path::Path, sync::Once, thread::yield_now};
    if !is_test_logging_enabled() {
        return;
    }
    static LOG_CFG: [&str; 2] =
        ["../common/config/log_cfg_debug.yml", "../../common/config/log_cfg_debug.yml"];
    static INIT_LOG: Once = Once::new();
    for cfg in LOG_CFG {
        if Path::new(cfg).exists() {
            println!("Try init test log {}", cfg);
            INIT_LOG.call_once(|| {
                println!("Init test log {}", cfg);
                init_log(cfg)
            });
            while !INIT_LOG.is_completed() {
                yield_now()
            }
            return;
        }
    }
    panic!("No log configuration!")
}

// Some tests are tokio::test that do not need explicit runtime
#[allow(dead_code)]
pub fn init_test() -> tokio::runtime::Runtime {
    init_test_log();
    tokio::runtime::Runtime::new().unwrap()
}
