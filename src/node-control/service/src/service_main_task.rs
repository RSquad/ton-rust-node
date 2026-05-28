/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    audit::AuditLogFactory,
    elections::election_task::BindingStatusCallback,
    http::http_server_task,
    runtime_config::RuntimeConfigStore,
    task::{ContractsTask, ElectionsTask, VotingTask, task_manager::TaskController},
};
use anyhow::Context;
use common::{
    app_config::AppConfig, log::setup_log, snapshot::SnapshotStore,
    task_cancellation::CancellationCtx,
};
use std::{collections::HashMap, path::Path, sync::Arc};

pub async fn run(cancellation_ctx: CancellationCtx, config_path: String) {
    let app_cfg = Arc::new(match AppConfig::load(Path::new(&config_path)) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("Failed to open config file: {:#}", e);
            return;
        }
    });

    let _log_guard = match setup_log(Some(&app_cfg)) {
        Ok(guard) => guard,
        Err(e) => {
            eprintln!("Failed to setup log file: {:#}", e);
            return;
        }
    };

    tracing::info!("Service started");
    if let Err(e) = run_with_config(cancellation_ctx, app_cfg, config_path).await {
        tracing::error!("service error: {:#}", e);
    }
    tracing::info!("Service stopped");
}

pub async fn run_with_config(
    cancellation_ctx: CancellationCtx,
    app_cfg: Arc<AppConfig>,
    config_path: String,
) -> anyhow::Result<()> {
    let runtime_cfg = RuntimeConfigStore::initialize(app_cfg.clone(), config_path)
        .await
        .context("initialize runtime config store")?;
    let runtime_cfg = Arc::new(runtime_cfg);

    let audit = AuditLogFactory::from_config(&app_cfg.audit_log)
        .await
        .map_err(|e| anyhow::anyhow!("audit log init failed: {e}"))?;

    let store = Arc::new(SnapshotStore::new());

    // Status callback: when the elections runner detects binding status changes,
    // atomically update the runtime config and persist it to file.
    let cfg = runtime_cfg.clone();
    let on_status_change: BindingStatusCallback = Arc::new(move |statuses| {
        if let Err(e) = cfg.update_and_save(|app_cfg| {
            for (node_id, new_status) in &statuses {
                if let Some(binding) = app_cfg.bindings.get_mut(node_id) {
                    binding.status = *new_status;
                }
            }
        }) {
            tracing::error!("failed to persist binding status: {e:#}");
        }
    });

    let mut tasks = HashMap::new();
    tasks.insert(
        "contracts",
        Arc::new(TaskController::new(
            "contracts",
            ContractsTask::new(runtime_cfg.clone()),
            runtime_cfg.clone(),
        )),
    );

    tasks.insert(
        "elections",
        Arc::new(TaskController::new(
            "elections",
            ElectionsTask::new(
                runtime_cfg.clone(),
                store.clone(),
                Some(on_status_change),
                audit.clone(),
            ),
            runtime_cfg.clone(),
        )),
    );

    tasks.insert(
        "voting",
        Arc::new(TaskController::new(
            "voting",
            VotingTask::new(runtime_cfg.clone()),
            runtime_cfg.clone(),
        )),
    );

    let _ = tasks.get("contracts").expect("contracts task").enable().await;
    if app_cfg.elections.is_some() {
        let _ = tasks.get("elections").expect("elections task").enable().await;
    }
    if app_cfg.voting.is_some() {
        let _ = tasks.get("voting").expect("voting task").enable().await;
    }

    let config_changed = Arc::new(tokio::sync::Notify::new());

    let http_task_handle = tokio::spawn(http_server_task::run(
        cancellation_ctx.clone(),
        store.clone(),
        runtime_cfg.clone(),
        tasks.clone(),
        config_changed.clone(),
        audit.clone(),
    ));

    let max_wait = std::time::Duration::from_secs(10);
    let mut cancel = cancellation_ctx.subscribe();

    loop {
        let timeout = tokio::time::sleep(max_wait);
        tokio::pin!(timeout);

        tokio::select! {
            _ = &mut timeout => {
                // Reload config from file if changed externally
                if runtime_cfg.reload_from_file().await {
                    for task in tasks.values() {
                        let _ = task.restart().await;
                    }
                }
            }
            _ = config_changed.notified() => {
                // REST mutation changed structural config — rebuild caches.
                // Only restart tasks if caches are consistent; otherwise tasks
                // keep running against the previous caches.
                match runtime_cfg.force_reload().await {
                    Ok(()) => {
                        for task in tasks.values() {
                            let _ = task.restart().await;
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            "cache rebuild after config mutation failed; skipping task restart: {e:#}"
                        );
                    }
                }
            }
            _ = cancel.changed() => {
                break;
            }
        }
    }
    for task in tasks.values() {
        let _ = task.disable().await;
    }
    let _ = http_task_handle.await;
    Ok(())
}
