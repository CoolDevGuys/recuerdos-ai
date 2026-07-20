-- The cached, LLM-written profile digest, one row per user per domain.
--
-- A cache, not a system of record: every row here is reproducible from
-- the memories it was built from, and dropping the table costs one
-- regeneration per user rather than any data.
CREATE TABLE profile_digests (
    user_id      TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- 'coding' | 'personal'. Split so that saving a coding preference
    -- does not force the personal half to be rewritten too.
    domain       TEXT NOT NULL,
    -- The markdown. Empty is a real answer, meaning "nothing here is
    -- worth an assistant's attention" — cached so it is not re-asked on
    -- every session start.
    content      TEXT NOT NULL,
    -- Summarises the memories this digest was built from; the digest is
    -- stale when it no longer matches. Derived rather than a `dirty`
    -- flag every write site has to remember to set — see
    -- consolidation/domain/profile_digest.rs.
    fingerprint  TEXT NOT NULL,
    generated_at TEXT NOT NULL,
    PRIMARY KEY (user_id, domain)
);
