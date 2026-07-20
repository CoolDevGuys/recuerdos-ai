# Configuration

**Status: Phase 2.** Every key below is loaded and validated today.
`[auth]`, `[storage]`, `[embeddings]`, `[retrieval]` and
`[understanding.taxonomy].extra_categories` are enforced; the remaining
`[understanding]` and `[consolidation]` configure the model-backed
features; both degrade to something useful without a provider.

## Precedence

Defaults → `recordagent.toml` → `RECORDAGENT_*` env vars (env wins).
Nested keys use a double underscore: `RECORDAGENT_SERVER__PORT=8080` sets
`[server].port`.

```bash
recordagent init                 # writes ./recordagent.toml + creates the data dir
recordagent serve --config recordagent.toml
```

`init` refuses to overwrite an existing file, and re-loads what it wrote
before declaring success — if `init` reports success, the file is valid.

## Reference

See [recordagent.example.toml](../recordagent.example.toml) for every key
with its default and an explanatory comment. Highlights:

- `[embeddings].provider` defaults to `local` — embeddings work fully
  offline, no external service required.
- `[understanding].provider` defaults to `none` — extraction and
  reconciliation are opt-in, and with no provider configured there is zero
  LLM egress. Submitted content is stored verbatim instead; see
  [api.md](api.md#without-a-provider) for exactly what changes.
- `[consolidation].enabled` defaults to `true`. Expiry and importance
  decay run regardless of provider — both are arithmetic. Merging
  duplicates needs `[understanding].provider`, because deciding whether
  two memories mean the same thing has no offline fallback.
- `[storage].path` supports a leading `~` for `$HOME` expansion.

## Validation

Invalid config reports every problem at once, not just the first:

```
config error: [server].port is 0; [embeddings].model is empty
```

Each message names the exact TOML path it's complaining about.

## Authentication

```toml
[auth]
mode = "api-key"   # api-key | none
```

### `mode = "api-key"` (default)

Every `/v1` route requires a bearer key. Manage keys with the CLI:

```bash
recordagent user add alex --email alex@example.com
recordagent key issue --user alex --scopes read,write --name laptop
recordagent key list --user alex
recordagent key revoke b99f884a          # by prefix
```

The key is displayed once at issue time. Only an argon2id hash is stored,
so a lost key can be replaced but never recovered.

An unrecognised `mode` value falls back to `api-key`. Failing closed is
the only safe direction: a typo must never silently disable
authentication.

### `mode = "none"`

Disables authentication entirely — every request runs as a built-in user
named `default`. Intended for a single-user deployment bound to
`127.0.0.1`, where the OS is already the access control.

Anyone who can reach the port is the `default` user. The server logs a
warning at startup whenever this is set.

Turning it back on later is safe: `default` is a real persisted user, so
memories written while auth was off keep their owner rather than being
orphaned.

## Embeddings

```toml
[embeddings]
provider  = "local"                    # only "local" works today
model     = "bge-small-en-v1.5"        # or all-minilm-l6-v2
cache_dir = "~/.recordagent/models"
```

Embeddings are computed in-process, on CPU, with no external service —
that is why the daemon works with no API key and no network.

`cache_dir` is where the ~130 MB model lives. The Docker image bakes it
in at `/models`, so containers never download at runtime. On bare metal
the model is fetched on first use; `recordagent warm-models` does it
ahead of time, which is what you want before taking a host offline.

**The model is pinned per collection.** Vectors from two different models
are not comparable, so changing `model` against an existing database is
refused with an actionable error rather than silently returning nonsense
rankings. Both supported models are 384-dimensional.

## Retrieval

```toml
[retrieval]
hybrid                 = true
default_limit          = 8
recency_half_life_days = 90
```

`recency_half_life_days` tunes how much a memory's age discounts it.
Recency is a *bounded* multiplier — it can reorder comparably relevant
results but never bury a far more relevant old memory. See the reasoning
in `src/memories/domain/recall_ranker.rs`.

`default_limit` applies when a search omits `limit`; requests are capped
at 50 regardless.

## Storage

```toml
[storage]
path = "~/.recordagent/data"
```

The database lives at `<path>/recordagent.db`, and the per-user keyword
indexes at `<path>/text-index/<user-id>/`. Both are created, along with
any missing parent directories, on first use. Schema migrations run
automatically at startup and are idempotent. Backup is `cp` of the data
directory (stop the daemon first, or copy the WAL files with it).

## Consolidation

```toml
[consolidation]
enabled = true
schedule = "daily"              # hourly | daily | weekly
similarity_threshold = 0.92
```

The nightly tidy-up. Three things happen in one pass, per user:

1. **Expiry.** Memories past their `expires_at` are retired — soft
   deleted, with an audit entry naming the date they hit. `expires_at` is
   a promise that a memory stops being *used*, not that it stops
   existing.
2. **Decay.** `importance` is recomputed from how recently and how often
   each memory was actually recalled, and feeds recall ranking as a
   bounded multiplier. It only ever demotes, and never below a floor well
   above zero: a decision from last year that nobody has looked at since
   must lose ties, not disappear.
3. **Merging.** Near-duplicates within one user's category are grouped
   and replaced by the single memory that says what all of them said. The
   originals are superseded, not deleted, each with a `merge` audit entry
   carrying the model's reasoning.

Only the third needs a provider.

### Running it by hand

```bash
recordagent consolidate --dry-run   # report what would merge; changes nothing
recordagent consolidate             # apply
```

`--dry-run` calls no model, so it costs nothing to run. It prints each
cluster's contents rather than its ids, because the point is for a person
to judge whether the grouping is right.

### `schedule`

An interval from process start, not a wall-clock time. A cron expression
would let you pick 3am, which sounds better than it is: RecordAgent runs
on laptops, and a laptop is asleep at 3am. The first run is one interval
*after* startup, so a daemon that restarts often does not consolidate —
and pay for it — every time.

### `similarity_threshold`

How alike two memories must be to be *considered* the same thing, as
cosine similarity of their embeddings. High on purpose: it is the only
thing standing between a chain of loosely-related memories and a merge
that loses a fact.

Passing the threshold only earns a memory a place in a cluster. The model
gets the last word and is prompted to decline when unsure — "prefers
pnpm" and "prefers Vitest" sit very close in embedding space and are two
different true things.

## The profile digest

`GET /v1/profile` and the MCP `memory://profile` resource return a
briefing on the user, capped at roughly 1500 tokens.

With `[understanding].provider` set, it is written by a model and cached
until the memories under it change — so it compresses rather than listing
and truncating. Staleness is detected by comparing the memories against
what the digest was built from, so no write path has to remember to
invalidate anything.

Without a provider, it is assembled from the highest-value memories per
category instead. Same route, same media type, same shape.
