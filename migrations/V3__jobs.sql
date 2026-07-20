-- Async ingestion jobs.
--
-- `POST /v1/memories` runs an LLM pipeline that takes seconds. Holding
-- the request open for it would make every client's timeout our problem
-- and would lose the work on a disconnect, so the request records the
-- intent here and returns immediately. Durability is the point: a job
-- accepted with 202 must survive a restart, or the API is lying about
-- having accepted it.

CREATE TABLE ingest_jobs (
    id      TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,

    -- The submission, verbatim, as JSON: content plus the source hints
    -- and any category/tags the caller suggested. Stored whole rather
    -- than split into columns because it is an opaque record of what was
    -- asked, and a failed job needs to be replayable exactly.
    payload TEXT NOT NULL,

    -- pending | running | succeeded | dead_letter
    --
    -- A retryable failure returns to `pending` with `run_after` pushed
    -- out; `dead_letter` is terminal and means a human should look.
    status   TEXT NOT NULL DEFAULT 'pending',
    attempts INTEGER NOT NULL DEFAULT 0,

    -- Why the last attempt failed. Kept even after a later attempt
    -- succeeds: "it worked eventually, but here is what went wrong" is
    -- the more useful record.
    error TEXT,

    -- Ids of the memories the job produced, as a JSON array. This is what
    -- makes `GET /v1/jobs/{id}` worth polling — a caller who submitted
    -- raw text learns which memories it became.
    memory_ids TEXT NOT NULL DEFAULT '[]',

    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,

    -- Not-before, for exponential backoff between attempts. A job is
    -- claimable only once the clock passes this.
    run_after TEXT NOT NULL,

    -- Set while a worker holds the job. A process killed mid-job leaves
    -- this set and `status = 'running'`; startup reclaims anything stale
    -- so a crash costs one retry rather than a permanently stuck row.
    claimed_at TEXT
);

-- The claim query: pending work whose backoff has elapsed, oldest first.
CREATE INDEX idx_ingest_jobs_claimable ON ingest_jobs (status, run_after);

-- Reclaiming after a crash scans running jobs by how long they have been
-- held.
CREATE INDEX idx_ingest_jobs_claimed ON ingest_jobs (status, claimed_at);

-- Callers poll their own jobs.
CREATE INDEX idx_ingest_jobs_user ON ingest_jobs (user_id, created_at);
