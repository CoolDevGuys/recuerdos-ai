-- V10__categories.sql
--
-- Each user's own taxonomy (ai-specs/web-ui-plan.md §4.2).
--
-- Until now a deployment widened the eight built-in categories through
-- `[understanding.taxonomy].extra_categories`: one list, in the config file,
-- shared by every user on the daemon. The web UI lets a user create, rename,
-- merge and delete categories of their own, so the list moves here.
--
-- One row per category a user has *touched*:
--   * every custom category (`built_in = 0`);
--   * a built-in only once it has been relabelled. Its id never changes —
--     built-in ids are what agents filter on and what the enriched vector
--     header is written from, so a relabel is display-only.
--
-- `label` NULL means "the default wording" (`Category::label`).
--
-- Seeding is lazy rather than done here, and recorded in `category_seeds`:
-- the config extras are not in the database for a migration to read. The
-- first time a user's catalog is read it is seeded with the configured
-- extras and with every custom category already on one of their memories,
-- so no stored memory ever names a category its owner cannot see or move.
-- Recording that the seed happened is what lets a user delete a seeded
-- category for good: without the marker the next read would bring it back.
CREATE TABLE categories (
    user_id    TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    id         TEXT NOT NULL,
    label      TEXT,
    built_in   INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (user_id, id)
);

CREATE TABLE category_seeds (
    user_id   TEXT PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    seeded_at TEXT NOT NULL
);
