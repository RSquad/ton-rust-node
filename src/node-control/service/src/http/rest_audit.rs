/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::http_server_task::AppState;
use crate::{
    audit::{AuditEvent, ConfigFieldChange, actor_builder::client_ip_from_headers},
    auth::Claims,
};
use common::app_config::{ContractsAutomationConfig, ElectionsConfig};

pub fn config_field(
    field: impl Into<String>,
    old: serde_json::Value,
    new: serde_json::Value,
) -> ConfigFieldChange {
    ConfigFieldChange { field: field.into(), old, new }
}

pub async fn record_login_success(
    state: &AppState,
    username: &str,
    role: &str,
    headers: &axum::http::HeaderMap,
) {
    let actor = state.actor_builder.rest_user(username, role, client_ip_from_headers(headers));
    state.audit.record(AuditEvent::rest_api_auth_login_success(actor, username)).await;
}

pub async fn record_login_rejected(
    state: &AppState,
    username: &str,
    reason: &str,
    headers: &axum::http::HeaderMap,
) {
    let actor = state.actor_builder.rest_user(username, "unknown", client_ip_from_headers(headers));
    state.audit.record(AuditEvent::rest_api_auth_login_rejected(actor, username, reason)).await;
}

pub async fn record_token_rejected(
    state: &AppState,
    user_id: &str,
    reason: &str,
    headers: &axum::http::HeaderMap,
) {
    let actor = state.actor_builder.rest_user(user_id, "unknown", client_ip_from_headers(headers));
    state.audit.record(AuditEvent::rest_api_token_rejected(actor, user_id, reason)).await;
}

pub async fn record_config_updated(
    state: &AppState,
    claims: &Claims,
    headers: &axum::http::HeaderMap,
    config_id: &str,
    operation: &str,
    changes: Vec<ConfigFieldChange>,
) {
    // Intentional: audit records field-level diffs only, not bare update attempts.
    // A request that results in zero tracked changes (e.g. exclude with an empty
    // node list) emits no `rest_api.config_updated` event.
    if changes.is_empty() {
        return;
    }
    let actor = state.actor_builder.rest_user(
        &claims.sub,
        claims.role.to_string(),
        client_ip_from_headers(headers),
    );
    state
        .audit
        .record(AuditEvent::rest_api_config_updated(actor, config_id, operation, changes))
        .await;
}

pub async fn record_entity_mutation(
    state: &AppState,
    claims: &Claims,
    headers: &axum::http::HeaderMap,
    config_id: &str,
    operation: &str,
    field: impl Into<String>,
    old: serde_json::Value,
    new: serde_json::Value,
) {
    record_config_updated(
        state,
        claims,
        headers,
        config_id,
        operation,
        vec![config_field(field, old, new)],
    )
    .await;
}

pub fn elections_settings_changes(
    before: &ElectionsConfig,
    after: &ElectionsConfig,
    sleep_period_pct: Option<f64>,
    waiting_period_pct: Option<f64>,
    tick_interval: Option<u64>,
    max_factor: Option<f32>,
    policy_changed: bool,
    policy_node: Option<&str>,
) -> Vec<ConfigFieldChange> {
    let mut changes = Vec::new();
    if let Some(v) = sleep_period_pct {
        changes.push(config_field(
            "elections.sleep_period_pct",
            serde_json::json!(before.sleep_period_pct),
            serde_json::json!(v),
        ));
    }
    if let Some(v) = waiting_period_pct {
        changes.push(config_field(
            "elections.waiting_period_pct",
            serde_json::json!(before.waiting_period_pct),
            serde_json::json!(v),
        ));
    }
    if let Some(v) = tick_interval {
        changes.push(config_field(
            "elections.tick_interval",
            serde_json::json!(before.tick_interval),
            serde_json::json!(v),
        ));
    }
    if let Some(v) = max_factor {
        changes.push(config_field(
            "elections.max_factor",
            serde_json::json!(before.max_factor),
            serde_json::json!(v),
        ));
    }
    if policy_changed {
        let field = policy_node
            .map(|n| format!("elections.policy_overrides.{n}"))
            .unwrap_or_else(|| "elections.policy".into());
        let old =
            policy_node.and_then(|n| before.policy_overrides.get(n)).unwrap_or(&before.policy);
        let new = policy_node.and_then(|n| after.policy_overrides.get(n)).unwrap_or(&after.policy);
        changes.push(config_field(field, serde_json::json!(old), serde_json::json!(new)));
    }
    changes
}

pub fn automation_settings_changes(
    before: &ContractsAutomationConfig,
    req: &super::config_handlers::ContractsAutomationSettingsUpdateRequest,
) -> Vec<ConfigFieldChange> {
    let mut changes = Vec::new();
    if let Some(v) = req.tick_interval_sec {
        changes.push(config_field(
            "automation.tick_interval_sec",
            serde_json::json!(before.tick_interval_sec),
            serde_json::json!(v),
        ));
    }
    if let Some(v) = req.auto_deploy {
        changes.push(config_field(
            "automation.auto_deploy",
            serde_json::json!(before.auto_deploy),
            serde_json::json!(v),
        ));
    }
    if let Some(v) = req.auto_topup {
        changes.push(config_field(
            "automation.auto_topup",
            serde_json::json!(before.auto_topup),
            serde_json::json!(v),
        ));
    }
    if let Some(ref patch) = req.wallet {
        if let Some(v) = patch.deploy {
            changes.push(config_field(
                "automation.wallet.deploy",
                serde_json::json!(before.wallet.deploy),
                serde_json::json!(v),
            ));
        }
        if let Some(v) = patch.topup {
            changes.push(config_field(
                "automation.wallet.topup",
                serde_json::json!(before.wallet.topup),
                serde_json::json!(v),
            ));
        }
        if let Some(v) = patch.threshold {
            changes.push(config_field(
                "automation.wallet.threshold",
                serde_json::json!(before.wallet.threshold),
                serde_json::json!(v),
            ));
        }
    }
    if let Some(ref patch) = req.pool {
        if let Some(v) = patch.snp {
            changes.push(config_field(
                "automation.pool.snp",
                serde_json::json!(before.pool.snp),
                serde_json::json!(v),
            ));
        }
        if let Some(v) = patch.ton_core {
            changes.push(config_field(
                "automation.pool.ton_core",
                serde_json::json!(before.pool.ton_core),
                serde_json::json!(v),
            ));
        }
    }
    changes
}
