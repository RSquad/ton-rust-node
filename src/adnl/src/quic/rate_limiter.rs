use std::{
    collections::HashMap,
    net::IpAddr,
    sync::OnceLock,
    time::{Duration, Instant},
};

/// O(1) timestamp-based token bucket rate limiter.
///
/// Matches the C++ `RateLimiter` from `adnl/utils.hpp`
pub(crate) struct RateLimiter {
    period: f64,
    emission_interval: f64,
    ready_at: f64,
    last_take_at: Instant,
}

impl RateLimiter {
    /// Create a new rate limiter.
    ///
    /// - `capacity`: burst size (how many calls succeed in rapid succession).
    ///   Must be >= 1.
    /// - `period`: time in seconds between successive token emissions.
    ///   Must be > 0.
    pub fn new(capacity: u32, period: f64) -> Self {
        assert!(capacity >= 1, "RateLimiter capacity must be >= 1");
        assert!(period > 0.0, "RateLimiter period must be > 0");
        let emission_interval = (capacity - 1) as f64 * period;
        Self {
            period,
            emission_interval,
            ready_at: -emission_interval,
            last_take_at: Instant::now(),
        }
    }

    /// Try to consume one token. Returns `true` if allowed, `false` if rate-limited.
    pub fn take(&mut self) -> bool {
        let now = now_secs();
        self.last_take_at = Instant::now();
        // Clamp: don't accumulate more than `capacity` tokens worth of credit.
        let min_ready_at = now - self.emission_interval;
        if self.ready_at < min_ready_at {
            self.ready_at = min_ready_at;
        }
        if self.ready_at > now {
            return false;
        }
        self.ready_at += self.period;
        true
    }

    pub fn last_take_at(&self) -> Instant {
        self.last_take_at
    }
}

/// Per-IP rate limiter map with periodic cleanup.
///
/// Each unique IP gets its own `RateLimiter` instance, created on demand.
/// Stale entries (no activity for longer than `period` seconds) are evicted
/// every 10 seconds.
pub(crate) struct ConnectionRateLimiters {
    capacity: u32,
    period: f64,
    limiters: HashMap<IpAddr, RateLimiter>,
    cleanup_at: Option<Instant>,
}

impl ConnectionRateLimiters {
    pub fn new(capacity: u32, period: f64) -> Self {
        Self { capacity, period, limiters: HashMap::new(), cleanup_at: None }
    }

    /// Check per-IP rate limit. Returns `true` if allowed.
    /// If `capacity == 0`, rate limiting is disabled (always returns `true`).
    pub fn take_new_connection(&mut self, ip: IpAddr) -> bool {
        if self.capacity == 0 {
            return true;
        }
        let limiter =
            self.limiters.entry(ip).or_insert_with(|| RateLimiter::new(self.capacity, self.period));
        let allowed = limiter.take();
        self.schedule_cleanup();
        allowed
    }

    /// Remove stale per-IP entries. Call periodically from the accept loop.
    pub fn cleanup(&mut self) {
        let Some(at) = self.cleanup_at else { return };
        if Instant::now() < at {
            return;
        }
        let period = self.period;
        self.limiters.retain(|_, limiter| limiter.last_take_at().elapsed().as_secs_f64() < period);
        self.cleanup_at = if self.limiters.is_empty() {
            None
        } else {
            Some(Instant::now() + Duration::from_secs(10))
        };
    }

    fn schedule_cleanup(&mut self) {
        if self.cleanup_at.is_none() {
            self.cleanup_at = Some(Instant::now() + Duration::from_secs(10));
        }
    }
}

/// Configuration for QUIC connection rate limiting.
#[derive(Clone, Debug)]
pub struct QuicRateLimitConfig {
    /// Per-IP: max burst of new connections before throttling (0 = disabled).
    pub per_ip_capacity: u32,
    /// Per-IP: time in seconds to refill one token.
    pub per_ip_period: f64,
    /// Global: max burst of new connections across all IPs (0 = disabled).
    pub global_capacity: u32,
    /// Global: time in seconds to refill one token.
    pub global_period: f64,
    /// Whether to send QUIC Retry packets for unvalidated addresses.
    pub stateless_retry: bool,
}

impl Default for QuicRateLimitConfig {
    fn default() -> Self {
        Self {
            per_ip_capacity: 10,
            per_ip_period: 0.2,
            global_capacity: 100_000,
            global_period: 0.00001,
            stateless_retry: true,
        }
    }
}

impl QuicRateLimitConfig {
    /// Config that disables all rate limiting (for tests).
    pub fn disabled() -> Self {
        Self {
            per_ip_capacity: 0,
            per_ip_period: 1.0,
            global_capacity: 0,
            global_period: 1.0,
            stateless_retry: false,
        }
    }
}

/// Monotonic timestamp in seconds, used for the token bucket math.
/// Using a module-level function so tests can potentially override via mock.
fn now_secs() -> f64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);
    epoch.elapsed().as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    /// Advance the process-level monotonic clock by sleeping.
    /// For short durations this is reliable enough for unit tests.
    fn sleep_ms(ms: u64) {
        thread::sleep(Duration::from_millis(ms));
    }

    #[test]
    fn test_burst_and_refill() {
        // capacity=3, period=0.05s (50ms) → emission_interval = 2*0.05 = 0.1s
        let mut limiter = RateLimiter::new(3, 0.05);

        // Initial burst: 3 allowed
        assert!(limiter.take(), "1st should succeed");
        assert!(limiter.take(), "2nd should succeed");
        assert!(limiter.take(), "3rd should succeed");
        assert!(!limiter.take(), "4th should fail (burst exhausted)");

        // Wait one period → one token refills
        sleep_ms(55);
        assert!(limiter.take(), "should succeed after 1 period");
        assert!(!limiter.take(), "should fail again");

        // Wait two periods → two tokens refill
        sleep_ms(105);
        assert!(limiter.take(), "1st after 2 periods");
        assert!(limiter.take(), "2nd after 2 periods");
        assert!(!limiter.take(), "3rd should fail");

        // Wait long time → clamped to capacity (3)
        sleep_ms(500);
        assert!(limiter.take());
        assert!(limiter.take());
        assert!(limiter.take());
        assert!(!limiter.take());
    }

    #[test]
    fn test_capacity_one() {
        let mut limiter = RateLimiter::new(1, 0.05);

        assert!(limiter.take(), "first should succeed");
        assert!(!limiter.take(), "second should fail immediately");

        sleep_ms(55);
        assert!(limiter.take(), "should succeed after period");
        assert!(!limiter.take(), "should fail again");
    }

    #[test]
    fn test_per_ip_isolation() {
        let mut limiters = ConnectionRateLimiters::new(2, 0.05);

        let ip_a: IpAddr = "1.2.3.4".parse().unwrap();
        let ip_b: IpAddr = "5.6.7.8".parse().unwrap();

        assert!(limiters.take_new_connection(ip_a));
        assert!(limiters.take_new_connection(ip_a));
        assert!(!limiters.take_new_connection(ip_a), "ip_a exhausted");

        // ip_b is independent
        assert!(limiters.take_new_connection(ip_b));
        assert!(limiters.take_new_connection(ip_b));
        assert!(!limiters.take_new_connection(ip_b), "ip_b exhausted");
    }

    #[test]
    fn test_disabled_capacity_zero() {
        let mut limiters = ConnectionRateLimiters::new(0, 1.0);

        let ip: IpAddr = "1.2.3.4".parse().unwrap();
        for _ in 0..1000 {
            assert!(limiters.take_new_connection(ip));
        }
    }

    #[test]
    fn test_cleanup_stale() {
        // period=0.05s, so entries older than 50ms are stale
        let mut limiters = ConnectionRateLimiters::new(2, 0.05);

        let ip: IpAddr = "1.2.3.4".parse().unwrap();
        limiters.take_new_connection(ip);

        // Force cleanup_at to be in the past
        limiters.cleanup_at = Some(Instant::now());

        sleep_ms(60);
        limiters.cleanup();
        assert!(limiters.limiters.is_empty(), "stale entry should be evicted");
    }
}
