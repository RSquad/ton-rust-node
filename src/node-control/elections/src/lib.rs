/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */

use std::{collections::HashMap, sync::Arc};

use common::app_config::AppConfig;
use contracts::{NominatorWrapper, TonWallet};

/// Live `AppConfig` + wallet/pool caches from the service runtime (see `election_task::run`).
pub type RuntimeSnapshotFn = Arc<
    dyn Fn() -> (
        Arc<AppConfig>,
        Arc<HashMap<String, Arc<dyn TonWallet>>>,
        Arc<HashMap<String, Arc<dyn NominatorWrapper>>>,
    ) + Send
        + Sync,
>;

pub(crate) mod adaptive_strategy;
pub mod election_emulator;
pub mod election_task;
pub mod providers;
pub(crate) mod runner;
