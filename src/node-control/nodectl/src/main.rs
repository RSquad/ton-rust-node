/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::app_cli_args::AppCliArgs;
use commands::command_manager::CommandManager;
use common::{
    os_signals,
    task_cancellation::{CancellationCtx, CancellationReason},
};
use std::process::exit;

mod app_cli_args;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let args = AppCliArgs::parse()?;

    // Spawn signal handler task
    let mut cancellation_ctx = CancellationCtx::new();
    let signals_cancellation_ctx = cancellation_ctx.clone();
    let signal_handle = tokio::spawn(async move {
        os_signals::wait(signals_cancellation_ctx).await;
    });

    let cmd = args.command.as_ref().unwrap_or_else(|| {
        AppCliArgs::print_help();
        exit(0);
    });

    let run_handle = match CommandManager::execute(cmd, cancellation_ctx.clone()).await {
        Ok(h) => h,
        Err(e) => {
            anyhow::bail!("command failed: {:#}", e);
        }
    };

    // wait for the service to complete
    if let Some(run_handle) = run_handle {
        let _ = run_handle.await;
    }
    if !cancellation_ctx.is_cancelled() {
        cancellation_ctx.cancel(CancellationReason::GracefullyShutdown());
    }
    // Wait for signal handler to complete
    let _ = signal_handle.await;
    tracing::debug!("shutdown completed");
    Ok(())
}
