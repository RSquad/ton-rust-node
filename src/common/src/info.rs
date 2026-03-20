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
#[allow(dead_code)]
pub fn build_commit() -> Option<&'static str> {
    std::option_env!("BUILD_GIT_COMMIT")
}

#[allow(dead_code)]
pub fn is_test_logging_enabled() -> bool {
    if let Ok(skip_logs) = std::env::var("NODE_SKIP_TEST_LOGS") {
        if skip_logs.to_lowercase().as_str() == "yes" {
            return false;
        }
    }
    true
}
