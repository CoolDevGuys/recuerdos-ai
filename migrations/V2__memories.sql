-- Memories, their audit trail, and the per-user vector index.

-- A collection is a namespace within a user's memories. Phase 2 creates
-- exactly one ("main") per user on first write; the table exists now
-- because the embedding model is pinned *per collection* and a memory
-- must reference the collection whose vectors it can be compared against.
-- Mixing embedding models in one index silently corrupts similarity
-- search, so the pin is the point.
CREATE TABLE collections (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    embedding_model TEXT NOT NULL,
    dimensions      INTEGER NOT NULL,
    created_at      TEXT NOT NULL,
    UNIQUE (user_id, name)
);

CREATE TABLE memories (
    id            TEXT PRIMARY KEY,
    user_id       TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    collection_id TEXT NOT NULL REFERENCES collections (id) ON DELETE CASCADE,
    content       TEXT NOT NULL,
    category      TEXT NOT NULL,
    -- JSON arrays. Tags are queried with a LIKE over the canonical
    -- '["a","b"]' form; at personal scale that is far cheaper than the
    -- join table it would otherwise take, and Phase 2 always filters by
    -- user_id first. If tag filtering ever becomes hot, this is the thing
    -- to normalise.
    tags          TEXT NOT NULL DEFAULT '[]',
    entities      TEXT NOT NULL DEFAULT '[]',
    confidence    REAL NOT NULL DEFAULT 1.0,
    source_client     TEXT,
    source_session_id TEXT,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL,
    last_accessed_at TEXT,
    expires_at       TEXT,
    -- Set when a later memory replaces this one (Phase 4 reconciliation,
    -- Phase 5 consolidation). Superseded rows are retained and excluded
    -- from ordinary recall — supersede is not delete.
    superseded_by TEXT REFERENCES memories (id) ON DELETE SET NULL,
    -- Soft delete. The row survives so the audit trail stays truthful.
    deleted_at    TEXT
);

-- Recall always filters by user first, then by liveness.
CREATE INDEX idx_memories_user_active
    ON memories (user_id, deleted_at, superseded_by);
CREATE INDEX idx_memories_user_category ON memories (user_id, category);
CREATE INDEX idx_memories_user_created ON memories (user_id, created_at);

CREATE TABLE memory_audit (
    id        TEXT PRIMARY KEY,
    memory_id TEXT NOT NULL,
    user_id   TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    operation TEXT NOT NULL,
    -- Which client or use case performed it.
    actor     TEXT NOT NULL,
    detail    TEXT NOT NULL DEFAULT '',
    at        TEXT NOT NULL
);

-- Deliberately no FK to memories: the audit trail must outlive a hard
-- delete, or "what happened to that memory?" becomes unanswerable
-- exactly when it matters most.
CREATE INDEX idx_memory_audit_user_at ON memory_audit (user_id, at);
CREATE INDEX idx_memory_audit_memory ON memory_audit (memory_id);
