-- V6__consolidation_state.sql
--
-- Tracks when each user's (category, subcategory) group was last
-- consolidated, so a subsequent run can skip groups whose memories have
-- not changed. This is the "skip-unchanged" heuristic from Phase 7.1: a
-- group is skipped only when nothing in it was created, edited or
-- superseded since the last successful pass.
--
-- The key is (user_id, category, subcategory) rather than just
-- (user_id, category) because consolidation clusters within a
-- (category, subcategory) group (Phase 7.2) — the skip watermark has to
-- match that grain or it would compare a subcategory's max against a
-- whole category's and skip the wrong thing. `subcategory` is NOT NULL
-- with an empty-string sentinel for "no subcategory", so the primary key
-- behaves (SQLite treats NULLs in a PK as distinct, which would break the
-- upsert).
--
-- The `max_updated_at` column stores the maximum `updated_at` of the
-- group's active memories at the time of the last consolidation. On the
-- next run, the runner compares the current max `updated_at` against this
-- value — if they match, the group is unchanged and is skipped. Any
-- create, edit or supersede bumps a memory's `updated_at` above this
-- watermark; nightly rescoring (importance) and recall bookkeeping
-- (last_accessed_at / access_count) deliberately do not, so neither
-- defeats the skip.

CREATE TABLE consolidation_state (
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    category TEXT NOT NULL,
    subcategory TEXT NOT NULL DEFAULT '',
    -- The maximum updated_at among the group's active memories at the
    -- time of the last successful consolidation pass.
    max_updated_at TEXT NOT NULL,
    PRIMARY KEY (user_id, category, subcategory)
);
