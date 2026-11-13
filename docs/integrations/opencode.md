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
(global). There are two ways to connect. They serve the same tools; the difference is
whether the server knows **which project you are in**, which decides where "in this
repo we…" memories go and how recall ranks them. See
[mcp.md](../mcp.md#which-transport-stdio-or-http) for the full comparison.

### Option A — stdio shim (recommended for coding)

opencode spawns `recuerdos-ai mcp` in the project folder, and the shim detects the
project from the repository's git remote. Only the binary is needed on your machine
(`install.sh`, see [mcp.md](../mcp.md#on-a-new-computer)); the daemon can run on a
server:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "recuerdos-ai": {
      "type": "local",
      "command": ["recuerdos-ai", "mcp", "--client", "opencode"],
      "enabled": true,
      "environment": {
        "RECUERDOS_AI_API_KEY": "ra_live_…",
        "RECUERDOS_AI_URL": "http://127.0.0.1:7070"
      }
    }
  }
}
```

`--client opencode` is recorded as the source, so the audit trail distinguishes these
writes. `RECUERDOS_AI_URL` is the daemon's address; leave it out for `localhost:7070`.

### Option B — HTTP

Connect straight to the daemon's `/mcp` endpoint with the key as a bearer token.
Nothing but the daemon has to be installed. **There is no project detection over
HTTP**: a session is in no project unless you name it with the project headers, so
put this in the project's own `opencode.json`:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "recuerdos-ai": {
      "type": "remote",
      "url": "http://localhost:7070/mcp",
      "enabled": true,
      "headers": {
        "Authorization": "Bearer ra_live_…",
        "X-Recuerdos-Project": "quill",
        "X-Recuerdos-Project-Remote": "github.com/acme/quill"
      }
    }
  }
}
```

Without the two project headers, recall ranks every project's memories below your
own, and a project memory is saved only when the agent names the project. Memories
saved this way are recorded with the source `mcp-http` (the HTTP transport can't tell
one client from another). The endpoint accepts only loopback hosts by default — fine
for a local daemon; behind a proxy on a real hostname, terminate there.

If the `recuerdos-ai` binary lives only in a Docker container, you can spawn the shim
there (`"command": ["docker", "exec", "-e", "RECUERDOS_AI_API_KEY", "-i",
"recuerdos-ai", "recuerdos-ai", "mcp", "--client", "opencode"]`), but it then runs in
the container's folder, not your project's, so it detects no project. Prefer the
binary on your machine, or HTTP with the headers.

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
- The [`recuerdos-memory` skill](agent-skill.md) teaches the model when to read and
  save; install it where opencode looks for skills and add its `AGENTS.md` line.
- opencode has no session-start hook equivalent to Claude Code's, so the
  profile is read when the model chooses to read the
  `memory://profile` resource rather than unconditionally.
