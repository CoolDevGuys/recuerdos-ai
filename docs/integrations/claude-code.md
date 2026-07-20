# Claude Code integration

Give Claude Code a memory that survives `/clear`, new sessions, and other
projects.

## Prerequisites

A running daemon and an API key ([docs/mcp.md](../mcp.md#setup)):

```bash
recordagent serve &                      # leave running
recordagent user add alex
recordagent key issue --user alex --scopes read,write
```

## Configure the MCP server

Add RecordAgent to `~/.claude.json` (all projects) or a project's
`.mcp.json` (that project, shareable with your team):

```json
{
  "mcpServers": {
    "recordagent": {
      "command": "recordagent",
      "args": ["mcp", "--client", "claude-code"],
      "env": {
        "RECORDAGENT_API_KEY": "ra_live_…"
      }
    }
  }
}
```

`--client claude-code` is recorded as the source of every memory this
client saves, so `GET /v1/audit` can tell an editor's writes from a
script's.

If you run the daemon somewhere other than `localhost:7070`, add
`"RECORDAGENT_URL": "http://…"` to `env`.

Restart Claude Code, then check it connected:

```
/mcp
```

You should see `recordagent` with three tools. If not, see
[troubleshooting](../mcp.md#troubleshooting).

## Verify it works

Ask Claude Code, in one session:

> Remember that I forbid barrel files — no index.ts re-exports.

Then in a **new** session, in a **different** project:

> How should I structure imports in this project?

It should recall the preference without being told. That round trip —
across sessions and projects — is the whole point.

## Pull the profile at session start

The tools are only used when the model decides to use them. A hook makes
the profile unconditional, so standing preferences are in context before
the first message.

In `~/.claude/settings.json`:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "curl -sf -H \"Authorization: Bearer $RECORDAGENT_API_KEY\" http://127.0.0.1:7070/v1/profile || true"
          }
        ]
      }
    ]
  }
}
```

The hook's stdout becomes context for the session. `|| true` matters: if
the daemon is down, a failing hook should not stop you working.

This reads the same digest as the `memory://profile` resource, over REST,
because a hook is a shell command rather than an MCP client.

## Save a summary when the context compacts

Compaction is the moment a session's detail is about to be lost, which
makes it the natural moment to keep the durable part. A `PreCompact` hook
posts the summary to RecordAgent:

```json
{
  "hooks": {
    "PreCompact": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "jq -Rs '{content: ., category: \"experience\", client: \"claude-code\"}' | curl -sf -X POST -H \"Authorization: Bearer $RECORDAGENT_API_KEY\" -H 'Content-Type: application/json' -d @- http://127.0.0.1:7070/v1/memories:direct >/dev/null || true"
          }
        ]
      }
    ]
  }
}
```

**A caveat worth understanding before you enable this.** Today
`/v1/memories:direct` stores what it is given, verbatim — so this saves
the whole summary as one memory, which is coarse. Phase 4 adds
`POST /v1/memories`, which runs an extraction pipeline that pulls the two
or three durable facts out of a session and discards the rest. When that
lands, changing the URL is the entire upgrade.

Until then, prefer letting the model call `memory_save` deliberately: one
good memory beats a transcript.

## What to expect

Claude Code decides when to call the tools, guided by their descriptions.
In practice it will:

- call `memory_recall` when you ask something about your own conventions;
- call `memory_save` when you state a preference in a way that sounds
  durable ("always…", "never…", "we decided…");
- rarely call `memory_forget`, which is intended — its description
  actively discourages unprompted deletion.

If it saves too eagerly or not enough, that is the tool descriptions
doing their job badly rather than a config problem. They live in
`src/memories/infrastructure/mcp/memory_mcp_server.rs` as doc comments
and are meant to be tuned.

## Removing it

Delete the `recordagent` entry from your MCP config. Your memories stay
in the daemon's database; export them first with
`curl .../v1/memories/export` if you want them elsewhere.
