/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
/// TON Core pool message builders (same as [`crate::nominator::ton_core_pool`]).
pub use crate::nominator::ton_core_pool as messages;
/// Nominator pool contract implementation (wrapper, deploy state init, RPC).
mod wrapper;

pub use wrapper::{
    NominatorPoolWrapperImpl, ResolvedTonCorePool, resolve_deploy_pool_params,
    resolve_toncore_nominator_pools, resolve_toncore_pool,
};
