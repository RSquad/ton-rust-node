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
#![cfg_attr(feature = "ci_run", deny(warnings))]

#[macro_use]
pub mod stack;
#[macro_use]
pub mod executor;
pub mod error;
pub mod smart_contract_info;
pub mod utils;

pub use self::smart_contract_info::{run_smc_method, SmartContractInfo, SmcMethodResult};

include!("../../common/src/info.rs");
