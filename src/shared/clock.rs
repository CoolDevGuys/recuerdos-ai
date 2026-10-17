//! Time as a dependency, not a global. Every use case that needs "now"
//! takes `&dyn Clock` so tests can pin time instead of racing the wall clock.

use chrono::{DateTime, Utc};
use std::sync::{Arc, Mutex};

// Consumed starting with Phase 1+ use cases; not yet constructed by any
// Phase 0 code outside this module's own tests.
#[allow(dead_code)]
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[allow(dead_code)]
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

    /// Moves the clock to an absolute instant.
    ///
    /// [`advance`](Self::advance) answers "what time is it after the test
    /// waits"; this answers "what time was it when this happened", which is
    /// what a harness needs when it stamps a corpus with ages rather than
    /// running it forward (recall-quality-plan.md Task 0.1: the eval seeds
    /// each memory at `EVAL_NOW − age_days` and scores at `EVAL_NOW`).
    ///
    /// Clones share one instant, so the holder that sets it does not have to
    /// be the object that was handed to the code reading the clock.
    pub fn set(&self, instant: DateTime<Utc>) {
        *self.now.lock().expect("clock mutex poisoned") = instant;
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
    fn a_set_clock_moves_to_an_absolute_instant_and_clones_see_it() {
        // Task 0.1's mechanism: the eval holds one handle and stamps memories
        // by moving "now" backwards, while the saver and the recaller read the
        // same clock through the pipeline they were wired with.
        let epoch = DateTime::from_timestamp(0, 0).unwrap();
        let clock = FixedClock::at(epoch);
        let shared = clock.clone();

        clock.set(epoch - chrono::Duration::days(30));
        assert_eq!(clock.now(), epoch - chrono::Duration::days(30));
        assert_eq!(
            shared.now(),
            epoch - chrono::Duration::days(30),
            "a clone must share the instant, not copy it"
        );

        clock.set(epoch);
        assert_eq!(shared.now(), epoch, "moving back is as legal as moving on");
    }

    #[test]
    fn system_clock_reports_current_time() {
        let before = Utc::now();
        let clock = SystemClock;
        let now = clock.now();
        assert!(now >= before);
    }
}
