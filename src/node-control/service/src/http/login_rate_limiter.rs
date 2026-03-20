/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use std::collections::HashMap;

const LOGIN_ATTEMPT_WINDOW_SECS: u64 = 60;
const LOGIN_ATTEMPT_MAX_FAILURES: u32 = 5;
const LOGIN_ATTEMPT_BLOCK_SECS: u64 = 120;
const LOGIN_ATTEMPT_STALE_SECS: u64 = 900;
const MAX_LIMITER_ENTRIES: usize = 10_000;
const MAX_USERNAME_LEN: usize = 64;

#[derive(Default, Clone)]
/// In-memory limiter state keyed by `"<ip>:<username>"`.
/// Used by `/auth/login` to throttle repeated failed authentication attempts.
pub(crate) struct LoginRateLimiter {
    attempts: HashMap<String, LoginAttemptBucket>,
}

#[derive(Clone, Copy)]
/// Per-key counters and timestamps used to
/// rate limit login attempts.
struct LoginAttemptBucket {
    /// Number of failed login attempts within the current time window.
    failures: u32,
    /// Unix timestamp (seconds) when the current failure window started.
    window_started_at: u64,
    /// Unix timestamp (seconds) until which this key remains blocked.
    blocked_until: u64,
    /// Unix timestamp (seconds) of the last activity for this key.
    last_seen_at: u64,
}

impl LoginRateLimiter {
    pub(crate) fn is_blocked(&mut self, key: &str, now: u64) -> bool {
        self.cleanup(now);
        self.attempts.get(key).map(|b| b.blocked_until > now).unwrap_or(false)
    }

    /// Records a failed login attempt. Returns `Err` if the limiter is at capacity
    /// and cannot track new keys (the caller should treat this as a rate-limit rejection).
    pub(crate) fn record_failure(&mut self, key: &str, now: u64) -> Result<(), ()> {
        self.cleanup(now);

        if !self.attempts.contains_key(key) && self.attempts.len() >= MAX_LIMITER_ENTRIES {
            return Err(());
        }

        let bucket = self.attempts.entry(key.to_owned()).or_insert(LoginAttemptBucket {
            failures: 0,
            window_started_at: now,
            blocked_until: 0,
            last_seen_at: now,
        });

        if now.saturating_sub(bucket.window_started_at) > LOGIN_ATTEMPT_WINDOW_SECS {
            bucket.failures = 0;
            bucket.window_started_at = now;
        }

        bucket.failures = bucket.failures.saturating_add(1);
        bucket.last_seen_at = now;

        if bucket.failures >= LOGIN_ATTEMPT_MAX_FAILURES {
            bucket.blocked_until = now.saturating_add(LOGIN_ATTEMPT_BLOCK_SECS);
        }
        Ok(())
    }

    pub(crate) fn record_success(&mut self, key: &str) {
        self.attempts.remove(key);
    }

    fn cleanup(&mut self, now: u64) {
        self.attempts.retain(|_, b| {
            let is_blocked = b.blocked_until > now;
            is_blocked || now.saturating_sub(b.last_seen_at) <= LOGIN_ATTEMPT_STALE_SECS
        });
    }
}

/// Builds a rate-limit key in the form `"<ip>:<username>"`.
///
/// `x-forwarded-for` is typically present when traffic comes through a reverse proxy
/// or load balancer (nginx/ingress/cloud LB) that forwards client IP information.
/// If the header is missing, empty, or invalid, we fall back to `"unknown"` which
/// means requests without forwarded client IP share the same pseudo-IP bucket.
pub(crate) fn login_limiter_key(headers: &axum::http::HeaderMap, username: &str) -> String {
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("unknown");
    let username = truncate_username(username);
    format!("{ip}:{username}")
}

fn truncate_username(username: &str) -> &str {
    &username[..username.len().min(MAX_USERNAME_LEN)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    #[test]
    fn blocks_after_threshold_and_unblocks_after_duration() {
        let mut limiter = LoginRateLimiter::default();
        let key = "127.0.0.1:alice";
        let now = 1_000;

        for _ in 0..(LOGIN_ATTEMPT_MAX_FAILURES - 1) {
            limiter.record_failure(key, now).unwrap();
        }
        assert!(!limiter.is_blocked(key, now));
        limiter.record_failure(key, now).unwrap();

        assert!(limiter.is_blocked(key, now));
        assert!(limiter.is_blocked(key, now + LOGIN_ATTEMPT_BLOCK_SECS - 1));
        assert!(!limiter.is_blocked(key, now + LOGIN_ATTEMPT_BLOCK_SECS));
    }

    #[test]
    fn failure_counter_resets_after_window() {
        let mut limiter = LoginRateLimiter::default();
        let key = "127.0.0.1:bob";
        let now = 10;

        for _ in 0..(LOGIN_ATTEMPT_MAX_FAILURES - 1) {
            limiter.record_failure(key, now).unwrap();
        }

        // New attempt outside the counting window should start fresh.
        limiter.record_failure(key, now + LOGIN_ATTEMPT_WINDOW_SECS + 1).unwrap();
        assert!(!limiter.is_blocked(key, now + LOGIN_ATTEMPT_WINDOW_SECS + 1));
    }

    #[test]
    fn success_clears_failures() {
        let mut limiter = LoginRateLimiter::default();
        let key = "127.0.0.1:carol";
        let now = 100;

        for _ in 0..(LOGIN_ATTEMPT_MAX_FAILURES - 1) {
            limiter.record_failure(key, now).unwrap();
        }
        limiter.record_success(key);

        limiter.record_failure(key, now).unwrap();
        assert!(!limiter.is_blocked(key, now));
    }

    #[test]
    fn cleanup_removes_stale_entries() {
        let mut limiter = LoginRateLimiter::default();
        let key = "127.0.0.1:dave";
        let now = 200;

        limiter.record_failure(key, now).unwrap();
        assert!(limiter.attempts.contains_key(key));

        // Trigger cleanup with a newer operation.
        let later = now + LOGIN_ATTEMPT_STALE_SECS + 1;
        let _ = limiter.is_blocked("other:key", later);
        assert!(!limiter.attempts.contains_key(key));
    }

    #[test]
    fn key_uses_first_forwarded_ip_and_falls_back_to_unknown() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("10.0.0.1, 10.0.0.2"));
        assert_eq!(login_limiter_key(&headers, "alice"), "10.0.0.1:alice");

        let empty = HeaderMap::new();
        assert_eq!(login_limiter_key(&empty, "alice"), "unknown:alice");
    }

    #[test]
    fn truncate_username_short() {
        assert_eq!(truncate_username("alice"), "alice");
    }

    #[test]
    fn truncate_username_exact_limit() {
        let name = "a".repeat(MAX_USERNAME_LEN);
        assert_eq!(truncate_username(&name), name.as_str());
    }

    #[test]
    fn truncate_username_over_limit() {
        let long = "a".repeat(MAX_USERNAME_LEN + 20);
        let expected = "a".repeat(MAX_USERNAME_LEN);
        assert_eq!(truncate_username(&long), expected);
    }

    #[test]
    fn record_failure_rejects_when_at_capacity() {
        let mut limiter = LoginRateLimiter::default();
        let now = 1_000;

        for i in 0..MAX_LIMITER_ENTRIES {
            limiter.record_failure(&format!("ip:user{i}"), now).unwrap();
        }

        assert!(limiter.record_failure("ip:overflow", now).is_err());
        // Existing key still works
        assert!(limiter.record_failure("ip:user0", now).is_ok());
    }
}
