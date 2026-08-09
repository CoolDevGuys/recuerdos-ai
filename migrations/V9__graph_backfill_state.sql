-- V9__graph_backfill_state.sql
--
-- The resume watermark for `graph backfill --relations` (Task 7.3.5).
--
-- Relation backfill re-extracts relations for memories that have none, at
-- one model call each, bounded by `[graph].backfill_budget`. When the
-- budget stops it partway through a corpus, the next run must resume rather
-- than re-pay for everything already attempted — including memories that
-- yielded no relations, which have nothing on the graph to mark them done.
--
-- So this records, per user, the `created_at` of the newest memory whose
-- relations the backfill has attempted. The next run considers only
-- memories created after it. The cursor is advanced only after a memory has
-- been handled without error, in the same spirit as the consolidation
-- watermark (V6): a run stopped early on a budget limit leaves the cursor
-- at the last memory it actually finished.
--
-- Only `--relations` needs this. `--entities` is deterministic and free, so
-- re-running it is a harmless idempotent no-op rather than a repeated cost.
--
-- `relations_cursor` is an RFC 3339 timestamp, compared the same way the
-- edge tables compare `valid_from` — lexicographically, which is
-- chronological for the single fixed-offset format `created_at` is written
-- in.
CREATE TABLE graph_backfill_state (
    user_id          TEXT PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    relations_cursor TEXT NOT NULL
);
