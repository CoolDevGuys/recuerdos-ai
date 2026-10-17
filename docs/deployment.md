# Deploying on a server

This is the guide for running Recuerdos AI as a personal memory server —
one daemon on a machine you own, reachable by Claude Code, opencode,
Hermes and anything else, from wherever you are.

It assumes only that you can open a terminal on a Linux server (a $5/month
VPS is plenty) and that [Docker is installed](https://docs.docker.com/engine/install/).
No Rust, no build step.

The whole thing is four steps: run it, lock it, key it, connect it.

---

## 1. Run it

```bash
docker run -d --name recuerdos-ai \
  --restart unless-stopped \
  -p 127.0.0.1:7070:7070 \
  -v recuerdos-ai-data:/data \
  ghcr.io/cooldevguys/recuerdos-ai
```

What each part does:

- `--restart unless-stopped` — it comes back after a reboot or a crash.
- `-p 127.0.0.1:7070:7070` — binds to **localhost only**, so the port is
  not exposed to the internet directly. Step 2 puts HTTPS in front of it.
  (On a machine with no public network you can drop the `127.0.0.1:`.)
- `-v recuerdos-ai-data:/data` — all your memories live in this Docker
  volume. It survives `docker rm`, upgrades, everything but a manual
  delete. **This is the thing to back up** (step 5).

Notice there is **no `AUTH__MODE=none`** here — unlike the README's
laptop quickstart. On a server, auth stays on (the default). Every request
now needs an API key, which you make in step 3.

Check it started:

```bash
curl -s localhost:7070/healthz && echo   # -> ok
```

## 2. Put HTTPS in front of it

Your memories should not travel over plain HTTP, and the API key that
guards them definitely should not. The easiest way to get automatic HTTPS
is [Caddy](https://caddyserver.com/) — it fetches and renews a
certificate for you with no configuration to speak of.

Point a domain (e.g. `memory.example.com`) at your server, then create a
file called `Caddyfile`:

```
memory.example.com {
    reverse_proxy 127.0.0.1:7070 {
        # The MCP endpoint (/mcp) has a DNS-rebinding guard that only
        # accepts a loopback Host header. Rewrite it so proxied MCP
        # requests are accepted; without this, /mcp returns
        # "403 Forbidden: Host header is not allowed" (REST is unaffected).
        header_up Host localhost
    }
}
```

Run Caddy:

```bash
docker run -d --name caddy \
  --restart unless-stopped \
  --network host \
  -v "$PWD/Caddyfile:/etc/caddy/Caddyfile" \
  -v caddy-data:/data \
  caddy
```

That is it — `https://memory.example.com` now proxies to the daemon with
a valid certificate, renewed automatically. Every URL below uses your
domain.

> No domain yet? You can skip this step and reach the daemon over an SSH
> tunnel instead: `ssh -L 7070:localhost:7070 you@server`, then use
> `http://localhost:7070` from your laptop. Fine for one person; a domain
> + Caddy is nicer once you want it on your phone.

## 3. Make yourself a user and an API key

The key is minted by the CLI, which runs *inside* the container against
the same database:

```bash
docker exec recuerdos-ai recuerdos-ai user add alex
docker exec recuerdos-ai recuerdos-ai key issue --user alex --scopes read,write
```

The second command prints a key like `ra_live_…` **once**. Copy it now —
it is stored only as a hash and cannot be shown again. If you lose it,
issue another and revoke the old one:

```bash
docker exec recuerdos-ai recuerdos-ai key list --user alex
docker exec recuerdos-ai recuerdos-ai key revoke <prefix>
```

Test the key end to end:

```bash
curl -s https://memory.example.com/v1/profile \
  -H "Authorization: Bearer ra_live_…"
```

## 4. Connect your tools

Every client points at the same daemon with the same key, and they all
share one memory — which is the entire point. The easiest transport for a
remote daemon is **MCP over HTTP**, at `https://memory.example.com/mcp`.

**Claude Code** — in `~/.claude.json` or a project's `.mcp.json`:

```json
{
  "mcpServers": {
    "recuerdos-ai": {
      "type": "http",
      "url": "https://memory.example.com/mcp",
      "headers": { "Authorization": "Bearer ra_live_…" }
    }
  }
}
```

**opencode** — in `opencode.json`:

```json
{
  "mcp": {
    "recuerdos-ai": {
      "type": "remote",
      "url": "https://memory.example.com/mcp",
      "enabled": true,
      "headers": { "Authorization": "Bearer ra_live_…" }
    }
  }
}
```

**Hermes** and anything else: see the [integration recipes](integrations/).
For clients that only speak the REST API, the [plain-REST recipe](integrations/custom-agents.md)
and [api.md](api.md) have everything.

Full transport details, including verifying `/mcp` by hand with `curl`,
are in [mcp.md](mcp.md).

---

## 5. Back it up

Everything is one SQLite database in the `recuerdos-ai-data` volume. To
copy it out:

```bash
docker run --rm -v recuerdos-ai-data:/data -v "$PWD:/backup" \
  busybox tar czf /backup/recuerdos-ai-backup.tar.gz -C /data .
```

Put that on a schedule (cron, your provider's volume snapshots — whatever
you already trust). Restoring is the same command with `xzf` into a fresh
volume.

## 6. Upgrade

Take the backup from step 5 first — cheap insurance, and the one thing
you cannot redo afterwards. Then:

```bash
docker pull ghcr.io/cooldevguys/recuerdos-ai
docker stop recuerdos-ai && docker rm recuerdos-ai
# re-run the `docker run …` from step 1 — the volume (your data) is untouched
```

The storage format migrates itself forward on start; it has done so
cleanly across every release so far.

### What `v0.4.0` does on its first start

Upgrading from `v0.3.x` or earlier, two things happen before the daemon
serves its first request. Both are worth watching once, because the first
one prints:

- **The keyword index is rebuilt.** `v0.4.0` changed the BM25 analyzer (it
  no longer indexes stopwords like "what" and "the"), and the index lives in
  a new versioned directory, so an existing store's keyword leg is rebuilt
  from the `memories` table. It is derived state: nothing in the table is
  modified, and it makes no model calls — at personal scale, seconds.

  ```
  rebuilt the keyword index for 1 users (108 memories)
  ```

  If the rebuild fails, the start **fails with it** rather than serving a
  silently crippled keyword leg, and the message names the repair —
  `stop the daemon and run \`recuerdos-ai reindex --text\` with the same
  config`. Which is what to do: stop the container, rebuild against the same
  volume, start again:

  ```bash
  docker stop recuerdos-ai
  docker run --rm -v recuerdos-ai-data:/data \
    ghcr.io/cooldevguys/recuerdos-ai reindex --text
  ```

  The daemon has to be the one that is stopped, not the rebuild: run
  `reindex --text` while the daemon is up and tantivy refuses the writer,
  exactly as it should —

  ```
  error: internal error: failed to open a text index writer: Failed to
  acquire Lockfile: LockBusy. Some("Failed to acquire index lock. …")
  ```

  `reindex --text` is also the repair for an interrupted rebuild: it rebuilds
  unconditionally and is safe to repeat.

- **Nothing needs re-embedding.** Relevance is now calibrated per embedding
  model instead of assuming one constant, and that is a change to how stored
  vectors are *read*, not to the vectors: `v0.4.0` verified (`recuerdos-ai
  calibrate`, min/max/mean norms all `1.0000` at 3072 dims) that hosted
  embeddings arrive unit-normalised, so the new baseline needed no embedder
  change and **no reindex**. `recuerdos-ai reindex` remains what you run when
  you change the embedding *model*, nothing more.

  Do look at the number in force once after upgrading, if you use a hosted
  embedder: `recuerdos-ai calibrate` prints the baseline and the noise floor
  it measured, and [configuration.md](configuration.md) explains what the
  baseline does and what it looks like when it is wrong.

The reranker is not part of an upgrade: `mode = off` is the default, so
`v0.4.0` downloads no rerank model and loads no extra weights until you ask
for them. See [Optional: second-stage reranking](#optional-second-stage-reranking)
when you want it.

---

## Optional: turn on understanding and better embeddings

Out of the box everything runs locally with **zero network egress** —
embeddings are computed in-process and no LLM is called. That is a
complete, working memory server.

Configuring providers unlocks the smarter behaviour: **understanding**
(splitting raw text into atomic memories and superseding contradictions)
and, if you want it, **higher-quality embeddings** from a hosted model.
Both are opt-in and send your memories to that provider — an explicit
choice, [described in full](configuration.md).

The shape is: name the provider and the env var holding its key in a
config file, and pass both to the container.

```toml
# recuerdos-ai.toml
[understanding]
provider    = "gemini"          # anthropic | openai-compat | gemini | ollama
model       = "gemini-2.0-flash"
api_key_env = "GEMINI_API_KEY"
```

```bash
docker run -d --name recuerdos-ai \
  --restart unless-stopped \
  -p 127.0.0.1:7070:7070 \
  -v recuerdos-ai-data:/data \
  -v "$PWD/recuerdos-ai.toml:/recuerdos-ai.toml:ro" \
  -e RECUERDOS_AI_CONFIG=/recuerdos-ai.toml \
  -e GEMINI_API_KEY=your-real-key \
  ghcr.io/cooldevguys/recuerdos-ai
```

Two things worth repeating from [configuration.md](configuration.md),
because they trip people up:

- `api_key_env` holds the **name** of an environment variable, never the
  key itself. The key goes in `-e GEMINI_API_KEY=…` (or a secrets manager).
- After changing the embedding provider or model on a store that already
  has memories, run `docker exec recuerdos-ai recuerdos-ai reindex` (with
  the daemon stopped) to re-embed them. The daemon refuses to start on a
  model mismatch and tells you this.
- The entity/relation graph is on by default, so memories ingested from
  here carry graph edges. To give edges to memories that predate it — a
  store first filled with `[graph].enabled = false`, or upgraded from
  `v0.1.x` — backfill once: `docker exec recuerdos-ai recuerdos-ai graph
  backfill --entities --relations`. `--entities` makes no model calls;
  `--relations` re-extracts the missing edges, one bounded, resumable pass
  at a time (add `--dry-run` to see the scope and spend nothing). A store
  built with the graph on needs none of this.

Confirm what is actually in force at any time — no secrets printed:

```bash
docker exec recuerdos-ai recuerdos-ai config
```

---

## Optional: second-stage reranking

The recall legs embed the query and each memory *separately* and compare
the result. A cross-encoder does something the legs structurally cannot: it
reads the query and one candidate **together** and scores the pair, which
catches the memory that shares no words with the question and is not linked
in the graph either. On the eval set that is worth **+5.8 precision@1**.

It is opt-in, and deliberately stays that way. The stage costs a second ONNX
model on the request path, and the gain is real but not free:

| | Default (`bge-reranker-base`) | Smallest (`jina-reranker-v1-turbo-en`) |
|---|---|---|
| Weights | 1.1 GB | 147 MB |
| RAM per concurrent inference | ~1.5 GiB | ~180 MiB |
| Added recall latency (p50 / p95) | +59 / +71 ms | +16 / +20 ms |
| Measured precision@1 gain | **+5.8** | +1.9 (inside noise) |

There is no middle tier that keeps the gain: the small model is cheap because
it is weak, not because it is compact. If your box cannot hold 1.5 GiB for a
second stage, the honest setting is `mode = "off"`. Language is the other axis
to choose on: the default ranks English and Chinese, and the multilingual
candidates are costed beside it in
[configuration.md](configuration.md#language-what-the-default-model-actually-speaks).

### 1. Judge it on your own store first

The eval corpus is not your memories. `rerank audit` replays your queries
over your real store twice — as served today, and with the stage on — and
prints both orderings, how often the head moved, and what it cost. It writes
nothing (no access counters, no index writes), so **the daemon can stay
running** while it works, and it loads the model whatever `mode` says.

Write a query file — one query per line, or TOML blocks with an `expect`
substring naming the memory that *should* come first:

```toml
# queries.toml
[[query]]
text = "what database is behind the billing service"
expect = "ledger-db"

[[query]]
text = "does alex prefer short answers"
expect = "short answers"
```

```bash
docker run --rm -v recuerdos-ai-data:/data \
  ghcr.io/cooldevguys/recuerdos-ai rerank audit \
  --queries queries.toml --user alex
```

Read the `top-1 changed` count and the p@1/MRR before-and-after pair. If the
stage does not improve *your* queries on *your* store, it will not improve
your agent's either — that is a legitimate answer, and the eval found the
same about corpora where the fused order is already right.

Shadow mode is the same judgment on traffic you did not have to invent: set
`mode = "shadow"` and the daemon scores every real recall and logs one
`rerank_shadow` line per query (ids and ranks, never content) while serving
the old order. A week of that, then `grep -c 'changed=true'`. Details in
[`docs/evaluation.md`](evaluation.md).

### 2. Give it the weights

The model has to be on disk. `/models` inside the image is a **baked
directory, not a volume**, so an unbaked model is downloaded at first start,
into the container's own writable layer — lost on the next `docker rm`, and
fatal on a host with no network. Two ways to have it there:

**Bake it into the image** (what a personal deployment wants — no runtime
download, and the image is self-contained):

```bash
docker build -f docker/Dockerfile \
  --build-arg RERANK_MODEL=bge-reranker-base \
  -t recuerdos-ai:rerank .
```

That adds ~1.1 GB to the image. The default published image carries no rerank
model at all, because most deployments never turn the stage on and should not
pay for it.

**Or mount the model cache** so one download survives recreates (and can be
pre-warmed with the `warm-models` command):

```bash
docker run -d --name recuerdos-ai \
  --restart unless-stopped \
  -p 127.0.0.1:7070:7070 \
  -v recuerdos-ai-data:/data \
  -v recuerdos-ai-models:/models \
  ghcr.io/cooldevguys/recuerdos-ai
docker run --rm -v recuerdos-ai-models:/models \
  -e RECUERDOS_AI_RERANK__MODE=on \
  ghcr.io/cooldevguys/recuerdos-ai warm-models
```

If the stage is enabled and the weights are genuinely unavailable, startup
fails with the model named and the remedy in the message:

```
error: internal error: failed to load rerank model "bge-reranker-base"
(from BAAI/bge-reranker-base) into /models: Failed to retrieve model file:
onnx/model.onnx. If this host has no internet access, pre-populate the cache
with `recuerdos-ai warm-models` or bake the model into the image.
```

It never quietly falls back: a deployment that asked for a second stage and
cannot load one finds out at boot, not in the middle of a recall.

What it does *first* is say the wait is coming:

```
INFO loading the rerank model; if its weights are not in this cache yet, the first
     load downloads them (~1.1 GB for the default model) and can take minutes
```

A log that pauses there is a healthy pull, not a hang — `du -sh` on the cache
directory grows while it runs. The same applies to a bare binary rather than a
container, where the cache is whatever `[embeddings].cache_dir` says and must be
writable by the user the service runs as: under `DynamicUser=` or `ProtectHome=`
it often is not, and then the download has nowhere to land. Running
`recuerdos-ai warm-models` once as that user, with the same `--config`, takes
the wait off the boot path entirely.

### 3. Turn it on

```toml
[rerank]
mode = "on"
```

or, without editing the file, `-e RECUERDOS_AI_RERANK__MODE=on`. Restart, and
confirm the block expanded (it shows one line while off):

```bash
docker exec recuerdos-ai recuerdos-ai config
```

Expect recall latency to rise by roughly the table's numbers, idle RSS to
rise by the model's, and every other behaviour to be unchanged: a rerank
deadline overrun drops the stage and serves the fused order with a warning,
so the second stage can cost you milliseconds but never a failed request.

Turning it off is the same edit in reverse — `mode = "off"` and restart.
Nothing about it is persisted into your memories: a rerank score is a property
of one query against one candidate list, not of a stored memory, which is why
there is no "rerank everything" command and no migration to undo.

### 4. Confirm it is actually running

The sentence above — "a rerank deadline overrun drops the stage and serves the
fused order with a warning" — is the whole feature and the whole trap. Dropping
the stage is correct behaviour, and on a machine slower than the one that picked
the default deadline it is also what happens *every time*, which looks exactly
like `mode = "off"`. One request answers it:

```bash
curl -s $BASE/v1/memories/search \
  -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' \
  -d '{"query":"what are my programming preferences","limit":8}' | jq '.rerank'
```

```json
{"state": "applied", "judged": 10, "took_ms": 342}
```

If instead it reads `{"state": "skipped", "reason": "deadline", …}`, the stage is
loaded, judging, and being thrown away — and that is not a neutral
misconfiguration, because the wait is real: every recall pays the full deadline
to serve the order the stage never got to change, so a `mode = "on"` that always
skips is *slower* than `mode = "off"`.

Do not fix it by doubling the `took_ms` in that line. The head is scored in
one call, so a skipped leg reports the bound you already set, not what the
work costs; the number worth sizing against only appears once a leg reports
`applied`. Lower `[rerank].depth` or raise `timeout_ms` until one query comes
back `applied`, take `took_ms / judged` from *that*, and set the bound at
twice `depth ×` that rate. The journal says the same thing as a `WARN` on
each affected recall, so `journalctl -u recuerdos-ai | grep 'rerank leg
failed'` is the other way to see it — and note that `rerank audit` will *not*
show it, because the audit measures with no deadline on purpose. Sizing the
deadline against your own machine:
[configuration.md](configuration.md#sizing-timeout_ms-for-your-machine).

---

## A note on running more than one person

One API key is one user, and memories never cross between users. If you
want the store to yourself, one key is all you need. To let someone else
use the same daemon without seeing your memories, issue them their own
user and key — the isolation is enforced by the type system, not by
configuration you could get wrong. See [security.md](security.md).
