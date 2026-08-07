-- V8: the entity/relation graph (Strategy B, implementation-plan.md Task 7.3).
--
-- Two derived tables that let recall hop between memories over the
-- entities they share, rather than only matching a query against each
-- memory in isolation. Both are *projections* of data already on the
-- `memories` row (`entities` JSON, plus relations extracted from the same
-- LLM call): they carry no fact that could not be rebuilt from `memories`
-- with zero model calls, which is what makes an existing corpus
-- backfillable (Task 7.3.5) and what carrying `Entity` since Phase 4 was
-- for. Inert until `[graph].enabled` — recall does not read them until
-- Task 7.3.4.

-- The entities a memory mentions, canonicalised to an `entity_key` (see
-- `memories::domain::entity_key`) so `Fly.io`, `fly.io` and `Fly.io's`
-- land on one node. `name`/`kind` keep the first spelling seen, for
-- display. One row per (memory, key): the PK dedupes a memory that names
-- the same entity twice.
CREATE TABLE memory_entities (
    user_id    TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    memory_id  TEXT NOT NULL,
    entity_key TEXT NOT NULL,
    name       TEXT NOT NULL,
    kind       TEXT NOT NULL,
    PRIMARY KEY (user_id, memory_id, entity_key)
);

-- Seeding a hop starts from the entities named in a query, so the lookup
-- is "which memories mention this key", keyed by (user_id, entity_key).
CREATE INDEX idx_memory_entities_user_key ON memory_entities (user_id, entity_key);

-- Directed, bi-temporal relations: `subject —predicate→ object`, both
-- endpoints canonicalised to keys (with the original names kept for
-- display). Bi-temporality is the whole point of Strategy B, so the two
-- clocks are kept apart deliberately (implementation-plan.md Task 7.3,
-- decision 6):
--
--   * `memories.created_at/updated_at` is *transaction* time — when we
--     learned the fact.
--   * `valid_from`/`invalid_at` here is *valid* time — when the fact was
--     true. `invalid_at IS NULL` means "still true"; a superseding memory
--     sets it (Task 7.3.3) rather than deleting the row, so a query can
--     ask "what did we deploy on *before* the migration?".
--
-- `invalidated_by` records which memory closed the interval, for the
-- audit trail the supersede model promises.
--
-- Deliberately no foreign key to `memories`: a memory is soft-deleted
-- (its row survives), so a cascade would never fire, and the graph rows
-- are instead removed explicitly by `EntityGraph::remove` when a memory
-- is forgotten. The `users` cascade still cleans a whole account.
CREATE TABLE memory_relations (
    id             TEXT PRIMARY KEY,
    user_id        TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    memory_id      TEXT NOT NULL,
    subject_key    TEXT NOT NULL,
    predicate      TEXT NOT NULL,
    object_key     TEXT NOT NULL,
    subject_name   TEXT NOT NULL,
    object_name    TEXT NOT NULL,
    valid_from     TEXT NOT NULL,
    invalid_at     TEXT,
    invalidated_by TEXT
);

-- A hop walks edges from a frontier of keys, live at a point in time, in
-- either direction — so both endpoints are indexed with `invalid_at` to
-- keep the liveness filter cheap.
CREATE INDEX idx_memory_relations_subject ON memory_relations (user_id, subject_key, invalid_at);
CREATE INDEX idx_memory_relations_object ON memory_relations (user_id, object_key, invalid_at);
