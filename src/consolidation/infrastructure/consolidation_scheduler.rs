//! Runs consolidation on a timer.
//!
//! # Why an interval and not a cron expression
//!
//! `[consolidation].schedule` takes `daily`, `hourly` or `weekly` rather
//! than a cron string. A cron expression would let an operator pick 3am
//! local time, which sounds better than it is: RecordAgent runs on a
//! laptop as often as on a server, and a laptop is usually asleep at 3am.
//! An interval from process start fires on a machine that is actually
//! running, which is the property that matters for a job whose whole
//! purpose is to happen unattended.
//!
//! # Why it does not fire at startup
//!
//! The first run is one interval *after* the process starts. Consolidation
//! costs model calls, and a daemon that restarts often — a laptop, a
//! container being redeployed — would otherwise consolidate on every
//! restart and spend real money doing it.

use crate::consolidation::application::consolidation_runner::ConsolidationRunner;
use crate::shared::error::{RaError, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

pub const HOURLY: Duration = Duration::from_secs(60 * 60);
pub const DAILY: Duration = Duration::from_secs(24 * 60 * 60);
pub const WEEKLY: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The accepted values of `[consolidation].schedule`.
pub const SCHEDULES: &[&str] = &["hourly", "daily", "weekly"];

pub fn interval_for(schedule: &str) -> Result<Duration> {
    match schedule.trim().to_ascii_lowercase().as_str() {
        "hourly" => Ok(HOURLY),
        "daily" => Ok(DAILY),
        "weekly" => Ok(WEEKLY),
        other => Err(RaError::Validation(format!(
            "[consolidation].schedule {other:?} is not one of {}",
            SCHEDULES.join(", ")
        ))),
    }
}

/// Stops the scheduler and waits for a run in flight to finish.
pub struct SchedulerHandle {
    shutdown: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl SchedulerHandle {
    /// A run in progress is allowed to finish rather than being dropped
    /// mid-merge — the same reasoning as the ingest workers, and more
    /// pressing here: an abandoned merge could leave a replacement memory
    /// written with none of its cluster retired.
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = self.task.await;
    }
}

/// Starts the timer. Returns `None` when `[consolidation].enabled` is
/// false — an installation that would rather keep every memory it was
/// given, exactly as it was given.
pub fn start(
    runner: Arc<ConsolidationRunner>,
    enabled: bool,
    schedule: &str,
) -> Result<Option<SchedulerHandle>> {
    if !enabled {
        tracing::info!(
            "[consolidation].enabled = false: memories will not be de-duplicated \
             automatically. `recordagent consolidate --now` still works."
        );
        return Ok(None);
    }

    let period = interval_for(schedule)?;
    let (shutdown, mut stop) = watch::channel(false);

    let task = tokio::spawn(async move {
        // `interval` would fire immediately on its first tick; this
        // starts one period out. See the module docs.
        let mut timer = tokio::time::interval_at(tokio::time::Instant::now() + period, period);

        loop {
            tokio::select! {
                _ = timer.tick() => {
                    if let Err(error) = runner.execute(false).await {
                        // Never fatal. The next tick tries again, and a
                        // daemon that stopped consolidating because of one
                        // bad night would fail silently for weeks.
                        tracing::warn!(%error, "scheduled consolidation failed");
                    }
                }
                _ = stop.changed() => {
                    if *stop.borrow() {
                        return;
                    }
                }
            }
        }
    });

    tracing::info!(schedule, "consolidation scheduled");
    Ok(Some(SchedulerHandle { shutdown, task }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_documented_schedules_all_parse() {
        assert_eq!(interval_for("hourly").unwrap(), HOURLY);
        assert_eq!(interval_for("daily").unwrap(), DAILY);
        assert_eq!(interval_for("weekly").unwrap(), WEEKLY);
    }

    #[test]
    fn a_schedule_is_read_case_and_whitespace_insensitively() {
        assert_eq!(interval_for("  Daily ").unwrap(), DAILY);
    }

    #[test]
    fn an_unknown_schedule_names_the_ones_that_work() {
        // Config validation catches this first; the message still has to
        // be useful, because this is also reachable from the CLI path.
        let error = interval_for("0 3 * * *").unwrap_err();

        assert!(
            error.to_string().contains("hourly, daily, weekly"),
            "{error}"
        );
    }

    #[test]
    fn every_accepted_schedule_is_listed_for_the_operator() {
        // The list in the error message and the list config validates
        // against must not drift apart.
        for schedule in SCHEDULES {
            assert!(
                interval_for(schedule).is_ok(),
                "{schedule} is advertised but does not parse"
            );
        }
    }
}
