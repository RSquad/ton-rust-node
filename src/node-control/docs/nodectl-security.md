# Nodectl Security Guide (for operators)

This document explains how `nodectl` REST API security works in day-to-day operations.

## 1) Quick overview

- **Authentication is disabled by default.** All endpoints are accessible without a token until at least one user is created via `nodectl auth add`.
- On first start the service creates a JWT signing key in the vault (secret `auth.jwt-signing-key`) even when authentication is disabled.
- **No service restart is required** to enable authentication — the service hot-reloads the configuration, so adding a user activates auth immediately.
- A user logs in to the REST API with `username/password`.
- The API returns a JWT token with a limited lifetime.
- The token is sent in `Authorization: Bearer <token>`.
- Allowed actions are determined by user role.
- Token revocation is done via CLI (`nodectl auth ...`), not via REST API.

## 2) Roles and permissions

### `nominator`

Can:

- log in (`/auth/login`);
- view own identity (`/auth/me`);
- read status (`/v1/elections`, `/v1/validators`).

Cannot:

- change stake policy;
- control elections task state;
- include/exclude nodes;
- manage users.

### `operator`

Can:

- everything `nominator` can do;
- perform operational REST changes:
  - `/v1/elections/exclude`
  - `/v1/elections/include`
  - `/v1/stake_strategy`
  - `/v1/task/elections`
- view users list (`GET /auth/users`).

Cannot:

- create users via REST;
- delete users via REST;
- revoke tokens via REST.

### `nodectl admin` (infrastructure/admin host role)

This is **not** a REST role and not a JWT claim.  
It is a person/process with host/Pod access where `nodectl` can be executed.

Can:

- change auth config directly with `nodectl auth ...`;
- create/delete users through CLI/config path;
- revoke tokens (`nodectl auth revoke ...`);
- change default token TTLs.

Important:

- this path works directly via config/CLI;
- REST auth middleware is bypassed because access is at infrastructure level.

## 3) How to log in and use a token

### Get a token

Interactive:

- `nodectl api login <username>`
- `nodectl api login <username> --password-stdin` (non-interactive, reads password from stdin)

### Use a token

- store token in environment variable:
  - `export NODECTL_API_TOKEN="<jwt>"`
- run API commands:
  - `nodectl api elections`
  - `nodectl api validators`
  - `nodectl api task elections disable`

## 4) How to revoke tokens (CLI only)

Who can revoke tokens:

- `nodectl admin` (infrastructure role with host/Pod access and permission to run `nodectl`).
- `operator` cannot revoke tokens through REST API.

Revocation is implemented by setting `revoked_after` for a user.

Command:

- `nodectl auth revoke <username>`

Optional manual cutoff time:

- `nodectl auth revoke <username> --at <unix_timestamp>`

Effect:

- When you revoke a user, any token for that user with an issued-at time (`iat`) less than or equal to the set `revoked_after` timestamp will no longer be accepted. This means all previously issued tokens before or at the revocation cutoff are immediately invalidated, and only tokens created after the `revoked_after` time will be valid.

## 5) What the API validates on each protected request

For each Bearer token, checks are applied in this order:

1. header format is valid;
2. JWT signature and expiration (`exp`) are valid;
3. user exists in current config;
4. token role matches current user role;
5. revocation condition passes (`iat > revoked_after`, otherwise rejected);
6. role is sufficient for the requested endpoint.

If validation fails, response is `401` or `403`.

## 6) Login brute-force protection

`POST /auth/login` is protected by rate limiting:

- window: `60s`
- max failed attempts in window: `5`
- block duration after threshold: `120s`
- stale bucket cleanup: `900s`
- max tracked keys: `10000` (new attempts are rejected when at capacity)
- username truncated to `64` bytes in limiter key

Limiter key is `"<ip>:<username>"`, where `ip` is taken from
`x-forwarded-for` (first value).
If the header is missing/invalid, fallback key prefix is `unknown`.

Response codes:

- `401` for invalid credentials (before threshold);
- `429` when attempt threshold is exceeded.

## 7) Secure Kubernetes profile (single instance)

- run a single service replica;
- expose externally only through Ingress/LB;
- block direct access to Pod IP/NodePort;
- enforce TLS at Ingress;
- trust `x-forwarded-for` only when traffic is forced through trusted Ingress;
- store JWT key and password hashes in Vault;
- do not use `jwt_secret` fallback in production.

## 8) Logs and monitoring

Auth logs use `target="auth"` and structured fields for machine parsing.

Common fields:

- `event`: stable event name (primary key for dashboards/alerts)
- `status`: mapped HTTP status for auth decision paths (`401`, `429`, `500`)
- `reason`: normalized rejection reason (for example `invalid_credentials`, `revoked`)
- `user`: username when available
- `rate_limit_key`: limiter key for login throttling events
- `error`: backend/internal error details (server logs only)

Key events to monitor:

- `auth_login_rejected`:
  - `status=401`, `reason=invalid_credentials`
  - `status=429`, `reason=rate_limited` or `rate_limit_threshold_reached`
- `auth_token_rejected`:
  - reasons include `missing_user`, `user_lookup_error`, `role_mismatch`, `revoked`
- `auth_login_backend_error` (`status=500`)
- `auth_token_generation_error` (`status=500`)
- `auth_setup_failed` (startup/auth wiring issue)

Client responses remain sanitized (no internal backend details in API body).

Recommended alerts:

- spike in `event=auth_login_rejected` with `status=401`
- spike in `event=auth_login_rejected` with `status=429`
- frequent `event=auth_token_rejected` with `reason=role_mismatch` or `reason=revoked`
- any non-zero `event=auth_setup_failed`
