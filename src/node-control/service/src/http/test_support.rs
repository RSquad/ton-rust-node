/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Shared helpers for HTTP handler/integration tests — single place to build
//! [`AppState`] so field additions don't drift across test modules.

use crate::{
    audit::{AuditActorBuilder, InMemoryAuditLog, NoopAuditLog, log::AuditLog},
    auth::{jwt::JwtAuth, user_store::UserStore},
    http::{http_server_task::AppState, login_rate_limiter::LoginRateLimiter},
    runtime_config::{RuntimeConfig, RuntimeConfigStore},
    task::task_manager::{ServiceTask, TaskController},
};
use common::{app_config::AppConfig, snapshot::SnapshotStore, task_cancellation::CancellationCtx};
use std::sync::Arc;

pub const TEST_JWT_SECRET: &str = "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio="; // [42u8; 32]

pub struct NoopTask;

#[async_trait::async_trait]
impl ServiceTask for NoopTask {
    async fn run(&self, ctx: CancellationCtx, _app_config: Arc<AppConfig>) -> anyhow::Result<()> {
        let mut cancel = ctx.subscribe();
        let _ = cancel.changed().await;
        Ok(())
    }
}

pub fn elections_task(runtime_cfg: Arc<RuntimeConfigStore>) -> Arc<TaskController> {
    Arc::new(TaskController::new("elections", NoopTask, runtime_cfg))
}

pub async fn test_jwt_auth() -> Arc<JwtAuth> {
    Arc::new(JwtAuth::new(None, Some(TEST_JWT_SECRET)).await.unwrap())
}

/// Builds [`AppState`] with defaults: empty snapshot store and elections noop task.
pub async fn build_app_state(
    runtime_cfg: Arc<RuntimeConfigStore>,
    audit: Arc<dyn AuditLog>,
) -> AppState {
    build_app_state_with(runtime_cfg, audit, Arc::new(SnapshotStore::new()), None).await
}

/// Like [`build_app_state`], but allows overriding the snapshot store and elections task.
pub async fn build_app_state_with(
    runtime_cfg: Arc<RuntimeConfigStore>,
    audit: Arc<dyn AuditLog>,
    store: Arc<SnapshotStore>,
    elections_task_override: Option<Arc<TaskController>>,
) -> AppState {
    let elections_task =
        elections_task_override.unwrap_or_else(|| elections_task(runtime_cfg.clone()));
    let user_store = Arc::new(UserStore::new(runtime_cfg.clone() as Arc<dyn RuntimeConfig>));
    AppState {
        store,
        runtime_cfg: runtime_cfg.clone(),
        elections_task,
        jwt_auth: test_jwt_auth().await,
        user_store,
        login_rate_limiter: Arc::new(tokio::sync::Mutex::new(LoginRateLimiter::default())),
        config_changed: Arc::new(tokio::sync::Notify::new()),
        audit,
        actor_builder: Arc::new(AuditActorBuilder::new(runtime_cfg)),
    }
}

pub async fn build_app_state_from_config(cfg: AppConfig) -> AppState {
    let rt = Arc::new(RuntimeConfigStore::from_app_config(Arc::new(cfg)));
    build_app_state(rt, Arc::new(NoopAuditLog)).await
}

pub async fn build_app_state_audited(
    runtime_cfg: Arc<RuntimeConfigStore>,
) -> (AppState, Arc<InMemoryAuditLog>) {
    let audit_mem = Arc::new(InMemoryAuditLog::new());
    let state = build_app_state(runtime_cfg, audit_mem.clone()).await;
    (state, audit_mem)
}

pub async fn build_app_state_from_config_audited(
    cfg: AppConfig,
) -> (AppState, Arc<InMemoryAuditLog>) {
    let rt = Arc::new(RuntimeConfigStore::from_app_config(Arc::new(cfg)));
    build_app_state_audited(rt).await
}
