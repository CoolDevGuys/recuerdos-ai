# Security & isolation

**Status: Phase 2.** This describes what is enforced today; it grows with
each phase, and gets its full write-up in Phase 6. Anything not stated
here is not yet guaranteed.

The central promise of RecordAgent is in
[project-plan.md §11](../project-plan.md#11-multi-user-isolation--security):
**each user sees only their own memories.** This page explains how that is
enforced and how you can check it.

## Isolation

The guarantee has two halves, and both are tested.

### Compile time: `UserContext`

Every operation that touches user data requires a `UserContext` — a value
only successful authentication can produce. Its constructors are
`pub(in crate::identity)`, so no other context can invent one for a user
it did not authenticate. Reaching another user's data is not a mistake you
have to remember to avoid; it does not compile.

`scripts/check-boundaries.sh` additionally fails the build if anyone
widens that visibility or calls the constructors from outside `identity`
— the point is to catch someone "fixing" a compile error the wrong way.

### Runtime: the cross-tenant suite

`tests/identity_isolation.rs` asks one question repeatedly: can one user
reach another's data? It covers key resolution, credential forgery and
revocation blast radius, plus — since Phase 2 — memory recall, reads by
id, edits, deletes, export and the audit trail. Every later phase extends
it: Phase 3 for the MCP tools, Phase 4 for ingest jobs.

Two cases are worth calling out because they defeat the bugs a casual
test would miss:

- Recall is tested with **byte-identical content** stored by both users.
  A filter keyed on text rather than owner would pass any test that used
  distinct data.
- The vector index is tested with **identical vectors**. `user_id` is a
  vec0 partition key, so one user's query never scans another's vectors —
  isolation is a property of the index, not of remembering a `WHERE`.

Each user also gets their own tantivy index *directory*, so the keyword
leg has no shared postings to filter in the first place.

A read path that ships without a case in that file means the isolation
claim has quietly stopped being tested.

## API keys

Format: `ra_live_<8 hex prefix><32 hex secret>`, from the OS CSPRNG.

- **Only a hash is stored.** The secret half is hashed with argon2id
  (memory-hard, per-key random salt). A leaked database does not yield
  usable keys. A CLI test greps the raw `.db` file for an issued secret
  and fails if it appears.
- **The prefix is public.** It is stored in plaintext and indexed so
  authentication is one indexed lookup instead of an argon2 verify against
  every key in the table. It carries no authority: a key spliced from one
  user's prefix and another's secret is rejected (tested).
- **Shown once**, at issue time. Lost keys are replaced, not recovered.
- **Revocable**, by prefix. Revocation is idempotent and preserves the
  first revocation time — a later call cannot rewrite when access actually
  ended.

## Storage of memories

Memory content is stored in plaintext in SQLite, like everything else —
there is no encryption at rest (see the threat model below). Reads,
writes, edits and deletes all carry the authenticated user's id in their
`WHERE` clause, so naming another user's memory id affects zero rows.
`404` is returned identically whether a memory does not exist or belongs
to someone else; otherwise the API would be an oracle for other users'
ids.

Deletes are soft: the row is retained so the audit trail stays truthful,
and the memory stops being recalled. Erasing bytes is a governance
operation, deliberately not something an agent can trigger in passing.

## Key verification and caching

argon2id costs ~230 ms by design. Running it on every request made
authentication 96% of a response, so verified keys are cached for 5
minutes, keyed by a SHA-256 digest of the presented secret and compared
in constant time.

This is sound for API keys in a way it would not be for passwords: keys
are 128-bit random secrets, so the slow hash was never what stopped
online guessing — the entropy is. The slow hash protects a *stolen
database*, and the stored hash is unchanged.

What the cache does **not** shortcut, verified by tests:

- **Revocation.** The key row is read on every request, so a revoked key
  fails immediately rather than after the TTL.
- **Scopes.** Also read from the row every time.
- **The secret itself.** A valid prefix with a wrong secret is rejected;
  the cache holds a digest, never the plaintext.

## What errors reveal

Deliberately very little.

- Missing, malformed, unknown-prefix, wrong-secret and revoked keys all
  return the identical `401 {"error":{"code":"unauthorized","message":
  "invalid API key"}}`. A test asserts the messages are byte-equal across
  all of them, so a caller cannot learn which half of a guess was right.
- The argon2 verify runs *before* the revocation check, so a revoked key
  costs the same as an unknown one. Short-circuiting would make revocation
  detectable by response time.
- `500` responses always say exactly `"internal error"`. Paths, SQL and
  driver text go to the server log, keyed by `x-request-id`.

## Data locality

No telemetry, no update checks. Embeddings run locally by default, so
storage and search never touch the network.

The one thing that can leave the machine is the understanding pipeline,
and it is opt-in: `[understanding].provider` defaults to `none`. When you
do turn it on, what gets sent is the content you submitted plus, during
reconciliation, the handful of existing memories most similar to it — so
enabling a hosted provider means your memories are sent to it. The
`ollama` provider exists so that need not be true: it keeps the whole
pipeline on your own hardware.

API keys are never read from config files, only from the environment
variable `[understanding].api_key_env` names. A key pasted into
`recordagent.toml` gets committed eventually.

## Threat model

**In scope.** A caller with network access but no valid key; a caller with
a valid key for user A trying to read user B's data; an attacker holding a
copy of the database file.

**Out of scope today.** An attacker with write access to the data
directory or the host — SQLite has no encryption at rest, so the database
and its memory contents are exactly as protected as their file
permissions. Rate limiting and
brute-force lockout are not implemented; argon2's cost and a 128-bit
secret are what make guessing impractical. Hosting untrusted co-tenants is
a SaaS-phase concern (project-plan.md §15), not what this is hardened for.

**Recommended deployment.** Bind to `127.0.0.1` (the default) and reach it
over SSH or a private network. The daemon speaks plain HTTP, so if you
expose it, terminate TLS in front of it — otherwise bearer keys cross the
network in the clear.

## Reporting a problem

The repository is private during development. A disclosure address and
policy land with the public release (Phase 6).
