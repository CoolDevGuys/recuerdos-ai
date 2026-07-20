# Configuration

**Status: Phase 2.** Every key below is loaded and validated today.
`[auth]`, `[storage]`, `[embeddings]`, `[retrieval]` and
`[understanding.taxonomy].extra_categories` are enforced; the remaining
`[understanding]` and `[consolidation]` keys configure features that
arrive in Phases 4 and 5.

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
