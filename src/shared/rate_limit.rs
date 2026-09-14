//! Per-caller token buckets: the one line of defence against a client that
//! would otherwise spend everyone else's capacity.
//!
//! Deliberately not `tower_http`'s rate limiter: that is a *single global*
//! budget, so the loudest client decides everyone else's latency. In a
//! multi-user store that is the wrong trade — one agent in a save loop should
//! degrade its own throughput, not the whole daemon's.
//!
//! The bucket is continuous-refill rather than a fixed window, because a fixed
//! window lets a client double-spend across a boundary (a full window at
//! 11:59.9 and another at 12:00.1) while being coarser to honest bursts.
//!
//! # What it costs
//!
//! One mutex and a float per request. The lock is held for the length of a
//! HashMap lookup with no await inside, which is what makes a `std::sync::Mutex`
//! the right primitive here; `tokio::sync::Mutex` would add a task hop to the
//! cheapest part of the request.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Buckets idle for this long are forgotten.
///
/// A bucket at rest for a minute is already full, so dropping it costs its
/// owner nothing — while leaving them in place forever would let an attacker
/// grow the map without bound by presenting a fresh key per request.
const IDLE_EXPIRY: Duration = Duration::from_secs(60);

/// The most buckets kept at once. Beyond this, stale ones are swept on insert.
const MAX_TRACKED_CALLERS: usize = 10_000;

/// The verdict for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allowed,
    /// Out of tokens. `retry_after` is how long until one returns, rounded up
    /// to whole seconds for the `Retry-After` header.
    Denied {
        retry_after: Duration,
    },
}

struct Bucket {
    /// Tokens accumulated so far, capped at the burst size.
    tokens: f64,
    /// When it was last touched, and therefore when `tokens` was last updated.
    refreshed: Instant,
}

/// A token bucket per caller key.
pub struct RateLimiter {
    /// Time to earn one token.
    per_token: Duration,
    /// The ceiling, and the starting balance: a client that has been quiet is
    /// allowed to burst rather than being paced from zero.
    burst: f64,
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl RateLimiter {
    /// A limiter allowing `requests_per_minute` sustained, with bursts up to
    /// `burst`.
    ///
    /// `burst` below 1 would refuse everything, so it is clamped up: a
    /// misconfiguration should degrade into a slow service, not a dead one.
    pub fn new(requests_per_minute: u32, burst: u32) -> Self {
        let per_minute = requests_per_minute.max(1) as f64;
        Self {
            // 60_000 ms / rate = the interval one token represents.
            per_token: Duration::from_millis((60_000.0 / per_minute).ceil() as u64),
            burst: f64::from(burst.max(1)),
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Takes a token for `key`, refilling it first for the time it has been
    /// idle.
    ///
    /// `now` is a parameter rather than `Instant::now()` so the refill
    /// arithmetic is testable without sleeping.
    pub fn try_acquire(&self, key: &str, now: Instant) -> Decision {
        let mut buckets = self
            .buckets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let burst = self.burst;

        if buckets.len() >= MAX_TRACKED_CALLERS && !buckets.contains_key(key) {
            buckets
                .retain(|_, bucket| now.saturating_duration_since(bucket.refreshed) < IDLE_EXPIRY);
        }

        let bucket = buckets.entry(key.to_string()).or_insert(Bucket {
            tokens: burst,
            refreshed: now,
        });

        let elapsed = now.saturating_duration_since(bucket.refreshed);
        bucket.tokens =
            (bucket.tokens + elapsed.as_secs_f64() / self.per_token.as_secs_f64()).min(burst);
        bucket.refreshed = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            return Decision::Allowed;
        }

        // A caller with no tokens needs to wait for one, and only one: the
        // shortfall is under a token, so one interval is the honest answer.
        let retry_after = self
            .per_token
            .saturating_sub(Duration::from_secs_f64(bucket.tokens.max(0.0)));
        Decision::Denied {
            retry_after: round_up_to_second(retry_after),
        }
    }

    /// How many distinct callers are being tracked. Test-facing, and cheap
    /// enough to call there.
    #[cfg(test)]
    pub fn tracked_callers(&self) -> usize {
        self.buckets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}

fn round_up_to_second(elapsed: Duration) -> Duration {
    Duration::from_secs(elapsed.as_secs_f64().ceil().max(1.0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter(per_minute: u32, burst: u32) -> RateLimiter {
        RateLimiter::new(per_minute, burst)
    }

    #[test]
    fn a_quiet_caller_may_burst_up_to_the_limit() {
        let limiter = limiter(300, 3);
        let start = Instant::now();

        for _ in 0..3 {
            assert_eq!(limiter.try_acquire("alex", start), Decision::Allowed);
        }

        assert_eq!(
            limiter.try_acquire("alex", start),
            Decision::Denied {
                retry_after: Duration::from_secs(1)
            },
            "the fourth request in the same instant is one token short"
        );
    }

    #[test]
    fn tokens_come_back_at_the_sustained_rate() {
        // 60/min is one token per second.
        let limiter = limiter(60, 1);
        let start = Instant::now();
        assert_eq!(limiter.try_acquire("alex", start), Decision::Allowed);

        let half_a_second_later = start + Duration::from_millis(500);
        assert_eq!(
            limiter.try_acquire("alex", half_a_second_later),
            Decision::Denied {
                retry_after: Duration::from_secs(1)
            },
            "half a token is not a token"
        );

        let one_second_later = start + Duration::from_secs(1);
        assert_eq!(
            limiter.try_acquire("alex", one_second_later),
            Decision::Allowed
        );
    }

    #[test]
    fn one_caller_cannot_spend_another_s_budget() {
        let limiter = limiter(300, 1);
        let start = Instant::now();

        assert_eq!(limiter.try_acquire("alex", start), Decision::Allowed);
        assert_eq!(
            limiter.try_acquire("alex", start),
            Decision::Denied {
                retry_after: Duration::from_secs(1)
            }
        );
        assert_eq!(
            limiter.try_acquire("beatriz", start),
            Decision::Allowed,
            "a second caller sharing a daemon must not inherit the first's exhaustion"
        );
    }

    #[test]
    fn idling_does_not_bank_more_than_a_burst() {
        // Otherwise an agent that sleeps for a day gets to fire hundreds of
        // requests at once, which is the opposite of what the limiter is for.
        let limiter = limiter(300, 5);
        let start = Instant::now();
        let a_day_later = start + Duration::from_secs(86_400);

        for _ in 0..5 {
            assert_eq!(limiter.try_acquire("alex", a_day_later), Decision::Allowed);
        }
        assert_ne!(
            limiter.try_acquire("alex", a_day_later),
            Decision::Allowed,
            "a day idle should buy a burst, not an hour's worth"
        );
    }

    #[test]
    fn a_spendthrift_caller_is_told_when_to_come_back() {
        let limiter = limiter(30, 1);
        let start = Instant::now();
        assert_eq!(limiter.try_acquire("alex", start), Decision::Allowed);

        // Two tokens' worth short after spending everything an instant later.
        let denied = limiter.try_acquire("alex", start + Duration::from_millis(100));
        let Decision::Denied { retry_after } = denied else {
            panic!("expected a refusal, got {denied:?}");
        };
        assert_eq!(
            retry_after,
            Duration::from_secs(2),
            "2s per token at 30/min"
        );
    }

    #[test]
    fn a_misconfigured_limit_still_admits_something() {
        // `requests_per_minute = 0` and `burst = 0` are settable by typo. The
        // safe reading of a broken limiter is a slow service, not a dead one.
        let limiter = limiter(0, 0);
        assert_eq!(
            limiter.try_acquire("alex", Instant::now()),
            Decision::Allowed
        );
    }

    #[test]
    fn idle_buckets_are_forgotten_rather_than_accumulated() {
        let limiter = limiter(300, 5);
        let start = Instant::now();

        // Past the tracked ceiling, on keys that then go quiet.
        for index in 0..MAX_TRACKED_CALLERS + 10 {
            limiter.try_acquire(&format!("caller-{index}"), start);
        }
        let before = limiter.tracked_callers();
        assert!(before >= MAX_TRACKED_CALLERS, "{before}");

        // A new caller arriving later triggers the sweep of everything idle.
        limiter.try_acquire("late", start + IDLE_EXPIRY + Duration::from_secs(1));

        assert_eq!(
            limiter.tracked_callers(),
            1,
            "stale buckets must not survive: a caller per key is unbounded memory"
        );
    }
}
