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
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    #[error("Invalid argument: {0}")]
    InvalidArg(String),
    #[error("Invalid data: {0}")]
    InvalidData(String),
    #[error("Invalid operation: {0}")]
    InvalidOperation(String),
    #[error("{0}")]
    ValidatorReject(String),
    #[error("{0}")]
    ValidatorSoftReject(String),
    #[error("Timeout {0}")]
    Timeout(String),
}
