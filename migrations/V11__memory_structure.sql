-- V11__memory_structure.sql
--
-- Structured memories (ai-specs/memory-structure-plan.md §3, §6, §8).
--
-- A memory gains what its category used to fuse into one label, plus where it
-- applies:
--   kind        preference | rule | convention | decision | fact | experience
--               | learning | procedure | reference | goal | plan
--   topic       a built-in topic id (coding, tech, work, …) or one of the
--               user's own (`topics` below)
--   target      me | project | general
--   project_id  the project, when target = 'project' (`projects` below)
--   origin      user_said | user_confirmed | inferred | document
--   goal_status active | achieved | abandoned, for goals only
--   needs_review 1 when the structure was derived, not stated, and may be wrong
--
-- The columns are nullable here and filled by a startup backfill rather than
-- by this file: the old category → structure mapping reads tags and splits
-- user category ids, and keeping it in one place (Rust,
-- `memories::domain::legacy_structure`) means the backfill, a row read before
-- it ran, and every save until the save path takes a structure all agree. A
-- row the backfill has not reached is read through the same mapping, so no
-- reader ever sees a memory without a structure.
--
-- `category` and `subcategory` stay. Nothing is dropped: the old label
-- remains the record of what was originally stated, and `subcategory` is the
-- subtopic. `expires_at` is the plan's `valid_until`.

-- One row per project a user's memories apply to. A memory stores the id, so
-- renaming a project is one row and every memory follows.
CREATE TABLE projects (
    id         TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (user_id, name)
);

-- The identifiers a project is detected by: normalised git remotes
-- (`github.com/acme/quill`), folder names, and the names it had before a
-- rename, so older checkouts keep resolving.
CREATE TABLE project_aliases (
    user_id    TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    alias      TEXT NOT NULL,
    project_id TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    PRIMARY KEY (user_id, alias)
);

-- A user's own topics, and a label for a built-in one they relabelled. The
-- same shape as `categories` (V10), which these replace as the per-user list.
CREATE TABLE topics (
    user_id    TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    id         TEXT NOT NULL,
    label      TEXT,
    built_in   INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (user_id, id)
);

ALTER TABLE memories ADD COLUMN kind TEXT;
ALTER TABLE memories ADD COLUMN topic TEXT;
ALTER TABLE memories ADD COLUMN target TEXT;
ALTER TABLE memories ADD COLUMN project_id TEXT REFERENCES projects (id);
ALTER TABLE memories ADD COLUMN origin TEXT;
ALTER TABLE memories ADD COLUMN goal_status TEXT;
ALTER TABLE memories ADD COLUMN needs_review INTEGER NOT NULL DEFAULT 0;

-- Recall scopes by user, then by who a memory applies to.
CREATE INDEX idx_memories_user_scope ON memories (user_id, target, project_id);
-- The backfill's work list, and a cheap "is anything left?" check.
CREATE INDEX idx_memories_unstructured ON memories (kind) WHERE kind IS NULL;
