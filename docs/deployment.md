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

```bash
docker pull ghcr.io/cooldevguys/recuerdos-ai
docker stop recuerdos-ai && docker rm recuerdos-ai
# re-run the `docker run …` from step 1 — the volume (your data) is untouched
```

The storage format migrates itself forward on start; it has done so
cleanly across every release so far.

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

Confirm what is actually in force at any time — no secrets printed:

```bash
docker exec recuerdos-ai recuerdos-ai config
```

---

## A note on running more than one person

One API key is one user, and memories never cross between users. If you
want the store to yourself, one key is all you need. To let someone else
use the same daemon without seeing your memories, issue them their own
user and key — the isolation is enforced by the type system, not by
configuration you could get wrong. See [security.md](security.md).
