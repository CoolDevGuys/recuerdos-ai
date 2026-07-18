//! Time as a dependency, not a global. Every use case that needs "now"
//! takes `&dyn Clock` so tests can pin time instead of racing the wall clock.

use chrono::{DateTime, Utc};
use std::sync::{Arc, Mutex};

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Deterministic clock for tests: starts at a fixed instant and only moves
/// when told to. Exercised by domain/use-case tests starting Phase 1+; not
/// yet constructed by any Phase 0 code outside its own unit tests.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct FixedClock {
    now: Arc<Mutex<DateTime<Utc>>>,
}

#[allow(dead_code)]
impl FixedClock {
    pub fn at(instant: DateTime<Utc>) -> Self {
        Self {
            now: Arc::new(Mutex::new(instant)),
        }
    }

    pub fn advance(&self, delta: chrono::Duration) {
        let mut now = self.now.lock().expect("clock mutex poisoned");
        *now += delta;
    }
}

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().expect("clock mutex poisoned")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_clock_only_moves_on_advance() {
        let epoch = DateTime::from_timestamp(0, 0).unwrap();
        let clock = FixedClock::at(epoch);
        assert_eq!(clock.now(), epoch);
        clock.advance(chrono::Duration::days(1));
        assert_eq!(clock.now(), epoch + chrono::Duration::days(1));
    }

    #[test]
    fn system_clock_reports_current_time() {
        let before = Utc::now();
        let clock = SystemClock;
        let now = clock.now();
        assert!(now >= before);
    }
}
