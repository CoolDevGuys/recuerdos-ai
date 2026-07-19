-- Identity: users and their API keys.
--
-- Timestamps are RFC 3339 UTC strings. SQLite has no native datetime
-- type, and text keeps rows readable in `sqlite3` — worth more here than
-- the few bytes an integer epoch would save.

CREATE TABLE users (
    id         TEXT PRIMARY KEY,
    handle     TEXT NOT NULL UNIQUE,
    email      TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE api_keys (
    id          TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    -- Non-secret, indexed: authentication is one lookup on this column,
    -- not an argon2 verify against every key in the table.
    prefix      TEXT NOT NULL UNIQUE,
    -- argon2id hash of the secret half. The secret itself is never stored.
    secret_hash TEXT NOT NULL,
    -- Canonical comma-separated scope list, e.g. 'read,write'.
    scopes      TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    last_used_at TEXT,
    revoked_at  TEXT
);

CREATE INDEX idx_api_keys_user_id ON api_keys (user_id);
