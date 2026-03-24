# Nodectl Security Guide (for operators)

This document explains how `nodectl` REST API security works in day-to-day operations.

## 1) Quick overview

| State | `http.auth` section | Users | Result |
|-------|---------------------|-------|--------|
| **Locked (default)** | present | empty | All protected endpoints return `401`. No one can log in. |
| **Protected** | present | ≥ 1 | Endpoints require a valid JWT. Users log in with username/password. |
| **Open access** | removed (`null` or absent) | — | All endpoints are accessible without a token. |

- **Authentication is enabled by default.** A freshly generated config (`nodectl config generate`) includes the `http.auth` section with an empty user list — all protected endpoints return `401` until at least one user is created via `nodectl auth add`.
- On first start the service creates a JWT signing key in the vault (secret `auth.jwt-signing-key`).
- **No service restart is required** to enable or disable authentication — the service hot-reloads the configuration.
- A user logs in to the REST API with `username/password`.
- The API returns a JWT token with a limited lifetime.
- The token is sent in `Authorization: Bearer <token>`.
- Allowed actions are determined by user role.
- Token revocation is done via CLI (`nodectl auth ...`), not via REST API.

## 2) Three operational states

### Locked (default for new installations)

The generated config contains the `http.auth` section but no users:

```json
{
  "http": {
    "bind": "0.0.0.0:8080",
    "enable_swagger": true,
    "auth": {
      "operator_token_ttl": 2592000,
      "nominator_token_ttl": 86400,
      "min_password_length": 8
    }
  }
}
```

In this state every protected endpoint returns `401 Unauthorized`. The `/health` endpoint remains accessible. To unlock the API, create at least one user (see [Step-by-step](#step-by-step-enable-api-access)).

### Protected

After adding one or more users the API requires a valid JWT on every protected request. See [How to log in and use a token](#4-how-to-log-in-and-use-a-token).

### Open access (auth disabled)

To disable authentication and make all endpoints accessible without a token, explicitly remove the `http.auth` section from `config.json` (or set it to `null`):

```json
{
  "http": {
    "bind": "0.0.0.0:8080",
    "enable_swagger": true
  }
}
```

> **Warning:** Before disabling auth, make sure the API is not exposed externally. If `http.bind` is `0.0.0.0:…` and the service port is reachable from outside the pod/host, anyone can call any endpoint.

The service picks up the change automatically — no restart required.

### Step-by-step: enable API access

```bash
# 1. Create a user (inside the pod or on the host)
nodectl auth add -u <username> -r operator

# 2. Log in
nodectl api login <username>

# 3. Use the token
export NODECTL_API_TOKEN="<jwt>"
nodectl api elections
```

## 3) Roles and permissions

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

## 4) Token TTL (time-to-live)

Each role has an independent token lifetime. The TTL determines how long a JWT remains valid after login.

| Role | Default TTL | Seconds |
|------|-------------|---------|
| `operator` | 30 days | 2 592 000 |
| `nominator` | 1 day | 86 400 |

### View current TTLs

The values are stored in `http.auth` inside `config.json`:

```json
{
  "http": {
    "auth": {
      "operator_token_ttl": 2592000,
      "nominator_token_ttl": 86400
    }
  }
}
```

### Change TTLs

Use `nodectl auth set ttl`. Values can be plain seconds or human-friendly suffixes (`s`, `m`, `h`):

```bash
# Set operator TTL to 8 hours, nominator to 1 hour
nodectl auth set ttl --operator 8h --nominator 1h

# Set only operator TTL (nominator stays unchanged)
nodectl auth set ttl --operator 3600

# Set nominator TTL to 30 minutes
nodectl auth set ttl --nominator 30m
```

Changes take effect immediately — the service hot-reloads the config. Existing tokens keep their original expiration; new tokens issued after the change use the updated TTL.

## 5) How to log in and use a token

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

## 6) How to revoke tokens (CLI only)

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

## 7) What the API validates on each protected request

For each Bearer token, checks are applied in this order:

1. `http.auth` section exists in config — if missing, all routes pass through (open access);
2. header format is valid;
3. JWT signature and expiration (`exp`) are valid;
4. user exists in current config;
5. token role matches current user role;
6. revocation condition passes (`iat > revoked_after`, otherwise rejected);
7. role is sufficient for the requested endpoint.

If validation fails, response is `401` or `403`.

## 8) Login brute-force protection

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

## 9) TLS requirement for external access

nodectl serves plain HTTP — it does **not** terminate TLS. If the API is reachable from outside the pod or host, you **must** terminate TLS at the Ingress controller, load balancer, or reverse proxy in front of it.

Without TLS:

- Passwords sent to `POST /auth/login` travel in plain text and can be intercepted.
- JWT tokens in `Authorization: Bearer` headers travel in plain text. A captured token grants API access until it expires or is revoked.

> **Rule of thumb:** If traffic crosses a network boundary you do not fully control, encrypt it with TLS.

## 10) Secure Kubernetes profile (single instance)

- run a single service replica;
- expose externally only through Ingress/LB;
- block direct access to Pod IP/NodePort;
- enforce TLS at Ingress (see [TLS requirement](#9-tls-requirement-for-external-access));
- trust `x-forwarded-for` only when traffic is forced through trusted Ingress;
- store JWT key and password hashes in Vault;
- do not use `jwt_secret` fallback in production.

## 11) Logs and monitoring

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
