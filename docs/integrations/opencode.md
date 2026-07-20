# opencode integration

RecordAgent is a standard stdio MCP server, so opencode connects the same
way it connects to any other.

## Prerequisites

A running daemon and an API key ([docs/mcp.md](../mcp.md#setup)):

```bash
recordagent serve &
recordagent user add alex
recordagent key issue --user alex --scopes read,write
```

## Configure

In `opencode.json` (project) or `~/.config/opencode/opencode.json`
(global):

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "recordagent": {
      "type": "local",
      "command": ["recordagent", "mcp", "--client", "opencode"],
      "enabled": true,
      "environment": {
        "RECORDAGENT_API_KEY": "ra_live_…"
      }
    }
  }
}
```

`--client opencode` is recorded as the source of memories saved from
here, so the audit trail distinguishes them from other clients.

Add `"RECORDAGENT_URL"` to `environment` if the daemon is not on
`localhost:7070`.

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
