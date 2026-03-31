/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
/// Internal messages for nominator pool contract
pub mod messages;
/// Nominator pool contract implementation (wrapper, deploy state init, RPC).
mod ton_core_nominator;

pub use ton_core_nominator::{
    NominatorPoolWrapperImpl, resolve_deploy_pool_params,
};
