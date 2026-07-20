//! What a memory is worth now — the decay math, as a pure function.
//!
//! # The problem it solves
//!
//! A memory store that only ever grows treats a preference the user
//! states daily and a note they wrote once and never looked at again as
//! equally deserving of a context window. Confidence does not help: both
//! may be perfectly true. What separates them is use.
//!
//! # Why it only ever demotes
//!
//! Importance is a bounded multiplier on the recall score, floored well
//! above zero (see `MIN_IMPORTANCE`). A memory that decays all the way
//! is still findable — it just stops outranking things people actually
//! read. That is deliberate: the entire premise of the product is that
//! an architecture decision from last year is still worth having, and a
//! decay that could bury it would be a bug dressed as a feature.
//!
//! # Why two signals rather than one
//!
//! Frequency alone rewards a memory that was hammered once during a
//! migration and has been irrelevant ever since. Recency alone gives a
//! memory read once last Tuesday the same standing as one read fifty
//! times. Neither is the question being asked, which is "is this still
//! part of how this person works?" — so the two are averaged.

use chrono::{DateTime, Utc};

/// The lowest importance decay can produce.
///
/// Not zero, and not close to it. A memory at the floor still ranks; it
/// simply loses ties to memories in active use. Anything lower would
/// make "never recalled" functionally the same as "deleted", which is
/// not a judgement an automated job should be making.
pub const MIN_IMPORTANCE: f32 = 0.35;

/// How long since last use before the recency half of the score halves.
///
/// Deliberately long. Three months without reading a memory is entirely
/// normal for the kind of thing worth remembering — a deployment target,
/// a convention on a project between sprints.
pub const RECENCY_HALF_LIFE_DAYS: f32 = 90.0;

/// Accesses at which the frequency half of the score is ~halfway to its
/// maximum. Small, because real recall counts are small: a memory read
/// five times is genuinely in active use.
const FREQUENCY_SATURATION: f32 = 5.0;

/// Importance in `MIN_IMPORTANCE..=1.0`.
///
/// `last_accessed_at` of `None` means never recalled, in which case the
/// memory's own age stands in — a memory saved this morning has not
/// earned a demotion for not having been read yet.
pub fn importance(
    created_at: DateTime<Utc>,
    last_accessed_at: Option<DateTime<Utc>>,
    access_count: u32,
    now: DateTime<Utc>,
) -> f32 {
    let since = last_accessed_at.unwrap_or(created_at);
    let raw = 0.5 * recency(since, now) + 0.5 * frequency(access_count);

    MIN_IMPORTANCE + (1.0 - MIN_IMPORTANCE) * raw.clamp(0.0, 1.0)
}

/// Exponential decay from the last time the memory was used.
fn recency(since: DateTime<Utc>, now: DateTime<Utc>) -> f32 {
    // Clock skew, or a fixed clock in a test: never a bonus above 1.0.
    let days = ((now - since).num_seconds() as f32 / 86_400.0).max(0.0);
    0.5f32.powf(days / RECENCY_HALF_LIFE_DAYS)
}

/// Saturating: the difference between never and twice matters, the
/// difference between forty and fifty does not.
fn frequency(access_count: u32) -> f32 {
    1.0 - 0.5f32.powf(access_count as f32 / FREQUENCY_SATURATION)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp")
    }

    fn days_ago(days: i64) -> DateTime<Utc> {
        now() - Duration::days(days)
    }

    #[test]
    fn a_fresh_unread_memory_is_not_penalised_for_being_new() {
        // It has had no chance to be recalled. Demoting it would bury
        // every memory during the window it is most likely to matter.
        let fresh = importance(now(), None, 0, now());
        let old = importance(days_ago(365), None, 0, now());

        assert!(fresh > old, "fresh {fresh}, old {old}");
        assert!(fresh > 0.6, "a memory saved today scored {fresh}");
    }

    #[test]
    fn a_memory_in_active_use_beats_one_nobody_reads() {
        // The whole point of the feature.
        let used = importance(days_ago(365), Some(days_ago(2)), 20, now());
        let ignored = importance(days_ago(365), None, 0, now());

        assert!(used > ignored, "used {used}, ignored {ignored}");
    }

    #[test]
    fn nothing_ever_decays_below_the_floor() {
        // A memory that decayed to zero would be deleted in all but
        // name, which is not a call an unattended job gets to make.
        let ancient = importance(days_ago(10_000), Some(days_ago(10_000)), 0, now());

        assert!(ancient >= MIN_IMPORTANCE, "got {ancient}");
        assert!(
            ancient <= MIN_IMPORTANCE + 0.01,
            "a decade-old unread memory should sit at the floor, got {ancient}"
        );
    }

    #[test]
    fn importance_never_leaves_its_range() {
        let cases = [
            (days_ago(0), None, 0u32),
            (days_ago(0), Some(now()), u32::MAX),
            (days_ago(100_000), Some(days_ago(100_000)), 0),
            // Clock skew: "accessed in the future".
            (days_ago(0), Some(now() + Duration::days(30)), 5),
        ];

        for (created, accessed, count) in cases {
            let score = importance(created, accessed, count, now());
            assert!(
                (MIN_IMPORTANCE..=1.0).contains(&score),
                "created {created}, accessed {accessed:?}, count {count} gave {score}"
            );
        }
    }

    #[test]
    fn recency_decays_by_half_over_the_half_life() {
        // Pins the constant to observable behaviour: doubling the
        // half-life silently would change every ranking in the system.
        let recent = recency(now(), now());
        let one_half_life = recency(days_ago(RECENCY_HALF_LIFE_DAYS as i64), now());

        assert!((recent - 1.0).abs() < 1e-6);
        assert!((one_half_life - 0.5).abs() < 0.01, "got {one_half_life}");
    }

    #[test]
    fn frequency_saturates_rather_than_growing_without_bound() {
        // Otherwise one memory hammered during a migration outranks
        // everything else forever.
        let none = frequency(0);
        let some = frequency(5);
        let many = frequency(50);
        let absurd = frequency(5_000);

        assert_eq!(none, 0.0);
        assert!(some > none && many > some);
        assert!(many < 1.0 && absurd <= 1.0);
        assert!(
            (absurd - many).abs() < 0.01,
            "50 and 5000 accesses should be near-indistinguishable: {many} vs {absurd}"
        );
    }

    #[test]
    fn more_accesses_never_lower_importance() {
        // Monotonic in use, at fixed times — a property worth pinning,
        // since the two halves are combined by arithmetic that could
        // easily be got backwards.
        let mut previous = 0.0;
        for count in [0u32, 1, 2, 5, 10, 50] {
            let score = importance(days_ago(30), Some(days_ago(30)), count, now());
            assert!(
                score >= previous,
                "{count} accesses scored {score}, below the previous {previous}"
            );
            previous = score;
        }
    }

    #[test]
    fn a_longer_gap_since_use_never_raises_importance() {
        let mut previous = 1.0;
        for days in [0i64, 7, 30, 90, 365, 1_000] {
            let score = importance(days_ago(2_000), Some(days_ago(days)), 3, now());
            assert!(
                score <= previous,
                "{days} days since use scored {score}, above the previous {previous}"
            );
            previous = score;
        }
    }
}
