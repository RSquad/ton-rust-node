/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
pub mod task_macro;
pub mod task_manager;

use crate::{
    audit::log::AuditLog, elections::election_task::BindingStatusCallback,
    runtime_config::RuntimeConfig, task,
};
use common::snapshot::SnapshotStore;
use std::sync::Arc;

task!(VotingTask, crate::voting::voting_task::run {
   runtime_cfg: Arc<dyn RuntimeConfig>,
});

task!(ContractsTask, crate::contracts::contracts_task::run {
    runtime_cfg: Arc<dyn RuntimeConfig>,
});

task!(ElectionsTask, crate::elections::election_task::run {
    runtime_cfg: Arc<dyn RuntimeConfig>,
    store: Arc<SnapshotStore>,
    on_status_change: Option<BindingStatusCallback>,
    audit: Arc<dyn AuditLog>,
});
