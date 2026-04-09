/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::commands::cli_cmd::Commands;
use common::{log::setup_log, task_cancellation::CancellationCtx};

pub struct CommandManager {}

impl CommandManager {
    pub async fn execute(
        cmd: &Commands,
        cancellation_ctx: CancellationCtx,
    ) -> anyhow::Result<Option<tokio::task::JoinHandle<()>>> {
        let _log_guard = if !matches!(cmd, Commands::Service(_)) { setup_log(None)? } else { None };

        match &cmd {
            // TON HTTP API
            Commands::GetConfigParam(cmd) => {
                cmd.run().await?;
                Ok(None)
            }
            // REST API
            Commands::Api(cmd) => {
                cmd.run().await?;
                Ok(None)
            }
            // Configuration management
            Commands::Config(cmd) => {
                cmd.run(cancellation_ctx).await?;
                Ok(None)
            }
            // Deploy
            Commands::Deploy(cmd) => {
                cmd.run(cancellation_ctx).await?;
                Ok(None)
            }
            // Auth user management
            Commands::Auth(cmd) => {
                cmd.run().await?;
                Ok(None)
            }
            // Key management
            Commands::Key(cmd) => {
                cmd.run().await?;
                Ok(None)
            }
            // Service
            Commands::Service(cmd) => Ok(Some(cmd.run(cancellation_ctx).await?)),
            // Voting
            Commands::Vote(cmd) => {
                cmd.run().await?;
                Ok(None)
            }
        }
    }
}
