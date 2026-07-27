# opencode integration

Recuerdos AI is a standard stdio MCP server, so opencode connects the same
way it connects to any other.

## Prerequisites

A running daemon and an API key ([docs/mcp.md](../mcp.md#setup)):

```bash
recuerdos-ai serve &
recuerdos-ai user add alex
recuerdos-ai key issue --user alex --scopes read,write
```

## Configure

In `opencode.json` (project) or `~/.config/opencode/opencode.json`
(global). There are two ways to connect; **HTTP is simpler**, especially
for a daemon in Docker.

### Option A — HTTP (recommended)

Connect straight to the daemon's `/mcp` endpoint with the key as a bearer
token. Nothing but the daemon has to be installed — no `recuerdos-ai`
binary on your machine, no `docker exec`:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "recuerdos-ai": {
      "type": "remote",
      "url": "http://localhost:7070/mcp",
      "enabled": true,
      "headers": { "Authorization": "Bearer ra_live_…" }
    }
  }
}
```

Memories saved this way are recorded with the source `mcp-http` (the HTTP
transport can't tell one client from another). The endpoint accepts only
loopback hosts by default — fine for a local daemon; behind a proxy on a
real hostname, terminate there.

### Option B — stdio shim

If you'd rather spawn a local process (or your daemon is only reachable
by a command), point opencode at `recuerdos-ai mcp`:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "recuerdos-ai": {
      "type": "local",
      "command": ["recuerdos-ai", "mcp", "--client", "opencode"],
      "enabled": true,
      "environment": { "RECUERDOS_AI_API_KEY": "ra_live_…" }
    }
  }
}
```

`--client opencode` is recorded as the source, so the audit trail
distinguishes these writes. Add `"RECUERDOS_AI_URL"` to `environment` if
the daemon is not on `localhost:7070`.

If the `recuerdos-ai` binary lives only in a Docker container, spawn the
shim there instead:

```json
"command": ["docker", "exec", "-e", "RECUERDOS_AI_API_KEY", "-i", "recuerdos-ai", "recuerdos-ai", "mcp", "--client", "opencode"]
```

(with the key in `environment`, and the daemon container named
`recuerdos-ai`). Option A avoids all of this.

## Verify

Start opencode and ask something that requires a stored preference:

> What package manager should I use in this repo?

If nothing is stored yet, tell it one first — "remember that I prefer
pnpm" — then ask again in a fresh session.

## Sharing one memory store

Both opencode and Claude Code can use the **same key and the same
daemon**, which is the point: a preference you state in one is available
in the other. Use separate `--client` values so the audit trail still
distinguishes them.

Use separate *users* (and keys) only if you want the stores isolated —
for example a work identity and a personal one. Memories never cross
between users.

## Notes

- The tools are described in [docs/mcp.md](../mcp.md#tools). opencode
  decides when to call them from those descriptions.
- opencode has no session-start hook equivalent to Claude Code's, so the
  profile is read when the model chooses to read the
  `memory://profile` resource rather than unconditionally.
