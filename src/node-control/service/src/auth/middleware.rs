/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::{Claims, Role};
use crate::{
    http::{http_server_task::AppState, rest_audit},
    runtime_config::RuntimeConfig,
};
use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

fn unauthorized_response(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        axum::Json(serde_json::json!({
            "ok": false,
            "error": {"code": 401, "message": message}
        })),
    )
        .into_response()
}

pub async fn require_nominator(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    require_role_impl(state, req, next, Role::Nominator).await
}

pub async fn require_operator(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    require_role_impl(state, req, next, Role::Operator).await
}

async fn require_role_impl(
    state: AppState,
    mut req: Request<Body>,
    next: Next,
    min_role: Role,
) -> Response {
    // Check live config: when auth is not configured, pass through.
    // This allows auth to be enabled/disabled at runtime via config reload.
    {
        let cfg = state.runtime_cfg.get();
        if cfg.http.auth.is_none() {
            // Auth-disabled mode: inject a synthetic identity so mutation handlers
            // can still require `Extension<Claims>` without enabling JWT checks.
            //
            // `role` is set to `min_role` (the route gate), not a real user grant.
            // Audit events will show `sub: "anonymous"` with that role — intentional,
            // since there is no authenticated identity in this mode.
            req.extensions_mut().insert(Claims {
                sub: "anonymous".into(),
                role: min_role,
                iat: 0,
                exp: u64::MAX,
            });
            return next.run(req).await;
        }
    }

    let jwt_auth = &state.jwt_auth;

    let auth_header =
        req.headers().get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok());

    let token = match auth_header.and_then(|h| h.strip_prefix("Bearer ")) {
        Some(t) => t,
        None => {
            rest_audit::record_token_rejected(
                &state,
                "unknown",
                "missing_authorization",
                req.headers(),
            )
            .await;
            return unauthorized_response("missing or invalid Authorization header");
        }
    };

    let claims = match jwt_auth.verify(token) {
        Ok(c) => c,
        Err(e) => {
            let reason = e
                .downcast_ref::<jsonwebtoken::errors::Error>()
                .map(super::token_rejection_reason)
                .unwrap_or("invalid_token");
            rest_audit::record_token_rejected(&state, "unknown", reason, req.headers()).await;
            return unauthorized_response("invalid or expired token");
        }
    };

    let user = match state.user_store.find_user(&claims.sub) {
        Some(user) => user,
        None => {
            tracing::warn!(
                target: "auth",
                event = "auth_token_rejected",
                status = 401,
                reason = "missing_user",
                user = %claims.sub,
                "token rejected"
            );
            rest_audit::record_token_rejected(&state, &claims.sub, "missing_user", req.headers())
                .await;
            return unauthorized_response("invalid or expired token");
        }
    };

    if user.role != claims.role {
        tracing::warn!(
            target: "auth",
            event = "auth_token_rejected",
            status = 401,
            reason = "role_mismatch",
            user = %claims.sub,
            token_role = %claims.role,
            current_role = %user.role,
            "token rejected"
        );
        rest_audit::record_token_rejected(&state, &claims.sub, "role_mismatch", req.headers())
            .await;
        return unauthorized_response("invalid or expired token");
    }

    if let Some(revoked_after) = user.revoked_after {
        if claims.iat <= revoked_after {
            tracing::warn!(
                target: "auth",
                event = "auth_token_rejected",
                status = 401,
                reason = "revoked",
                user = %claims.sub,
                token_iat = claims.iat,
                revoked_after = revoked_after,
                "token rejected"
            );
            rest_audit::record_token_rejected(&state, &claims.sub, "revoked", req.headers()).await;
            return unauthorized_response("invalid or expired token");
        }
    }

    if claims.role < min_role {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({
                "ok": false,
                "error": {"code": 403, "message": "insufficient permissions"}
            })),
        )
            .into_response();
    }

    req.extensions_mut().insert(claims);
    next.run(req).await
}
