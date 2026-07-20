-- Importance decay: what a memory is worth now, rather than what it was
-- worth when it was written.
--
-- Both columns are derived, not asserted: `access_count` is bookkeeping
-- the recall path maintains, and `importance` is recomputed from it by
-- the nightly consolidation job. Neither is ever set by a client, and
-- neither is audited — an audit entry per memory per night would bury
-- the trail under changes nobody made.

-- How often this memory has actually been useful. Together with
-- `last_accessed_at` (V2) this is the whole input to decay: a memory
-- recalled twenty times last week matters more than one saved a year
-- ago and never read since.
ALTER TABLE memories ADD COLUMN access_count INTEGER NOT NULL DEFAULT 0;

-- The decay-weighted score, in 0.0..=1.0, used as a bounded multiplier
-- when ranking recall results.
--
-- Defaults to 1.0 rather than 0.0 so that memories written before this
-- migration — and memories written between one nightly run and the next
-- — rank exactly as they did before, instead of being buried until the
-- job first computes a real value for them. Decay only ever demotes; it
-- must never demote a memory nobody has measured yet.
ALTER TABLE memories ADD COLUMN importance REAL NOT NULL DEFAULT 1.0;
