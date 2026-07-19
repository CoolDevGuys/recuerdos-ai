# Security & isolation

**Status: Phase 1.** This describes what is enforced today; it grows with
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
reach another's data? Today it covers key resolution, credential forgery
and revocation blast radius. Every later phase extends it — Phase 2 for
memory recall, Phase 3 for the MCP tools, Phase 4 for ingestion.

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

Nothing leaves the machine in Phase 1 — no telemetry, no update checks, no
LLM providers. (Those arrive in Phase 4 and are opt-in:
`[understanding].provider` defaults to `none`.)

## Threat model

**In scope.** A caller with network access but no valid key; a caller with
a valid key for user A trying to read user B's data; an attacker holding a
copy of the database file.

**Out of scope today.** An attacker with write access to the data
directory or the host — SQLite has no encryption at rest, so the database
is exactly as protected as its file permissions. Rate limiting and
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
