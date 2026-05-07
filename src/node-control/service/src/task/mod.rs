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
    elections::election_task::BindingStatusCallback, runtime_config::RuntimeConfig, task,
    task::task_manager::ServiceTask,
};
use common::{app_config::AppConfig, snapshot::SnapshotStore, task_cancellation::CancellationCtx};
use std::sync::Arc;

task!(VotingTask, crate::voting::voting_task::run {
   runtime_cfg: Arc<dyn RuntimeConfig>,
});

task!(ContractsTask, crate::contracts::contracts_task::run {
    runtime_cfg: Arc<dyn RuntimeConfig>,
    store: Arc<SnapshotStore>
});

pub struct ElectionsTask {
    runtime_cfg: Arc<dyn RuntimeConfig>,
    store: Arc<SnapshotStore>,
    on_status_change: Option<BindingStatusCallback>,
}

impl ElectionsTask {
    pub fn new(
        runtime_cfg: Arc<dyn RuntimeConfig>,
        store: Arc<SnapshotStore>,
        on_status_change: Option<BindingStatusCallback>,
    ) -> Self {
        Self { runtime_cfg, store, on_status_change }
    }
}

#[async_trait::async_trait]
impl ServiceTask for ElectionsTask {
    async fn run(
        &self,
        cancellation_ctx: CancellationCtx,
        _app_config: Arc<AppConfig>,
    ) -> anyhow::Result<()> {
        crate::elections::election_task::run(
            cancellation_ctx,
            self.runtime_cfg.get(),
            self.runtime_cfg.rpc_client(),
            self.runtime_cfg.wallets(),
            self.runtime_cfg.pools(),
            self.store.clone(),
            self.runtime_cfg.vault(),
            self.on_status_change.clone(),
        )
        .await
    }
}
