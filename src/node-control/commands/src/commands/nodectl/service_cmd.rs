/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use common::task_cancellation::CancellationCtx;
use service::service_main_task;

#[derive(clap::Args, Clone)]
#[command(about = "Run Node Control as service")]
pub struct ServiceCmd {
    #[arg(
        short = 'c',
        long = "config",
        help = "Path to the configuration file",
        default_value = "nodectl-config.json",
        env = "CONFIG_PATH",
        global = true
    )]
    config: String,
}

impl ServiceCmd {
    pub async fn run(
        &self,
        cancellation_ctx: CancellationCtx,
    ) -> anyhow::Result<tokio::task::JoinHandle<()>> {
        tracing::info!("starting node control service");
        Ok(tokio::spawn(service_main_task::run(cancellation_ctx, self.config.clone())))
    }
}
