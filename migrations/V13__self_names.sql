-- V13__self_names.sql
--
-- Who "the user" is: the names the owner of a store goes by, first the one in
-- full. A memory that names them ("Alejandro Romero is a backend engineer…")
-- is about them, as one written "The user is…" is; without the names nothing
-- tells it from a memory about somebody else. Kept as a JSON array, read and
-- written whole.
CREATE TABLE self_names (
    user_id    TEXT PRIMARY KEY,
    names      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
