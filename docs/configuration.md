# Configuration

**Status: Phase 0.** Every key below is loaded and validated today. The
`[auth]` section is parsed and validated but not yet enforced by any
route — that lands in Phase 1.

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
- `[understanding].provider` defaults to `none` — extraction/labeling is
  opt-in; with no provider configured, zero LLM egress (this feature
  itself lands in Phase 4; today only the config key exists).
- `[storage].path` supports a leading `~` for `$HOME` expansion.

## Validation

Invalid config reports every problem at once, not just the first:

```
config error: [server].port is 0; [embeddings].model is empty
```

Each message names the exact TOML path it's complaining about.

## Coming in Phase 1

`[auth].mode` (`api-key` | `none`) gets enforced by an auth middleware
that resolves every request to a `UserContext`. See
[implementation-plan.md Phase 1](../implementation-plan.md#phase-1).
