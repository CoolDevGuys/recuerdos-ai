# Claude Code integration

Give Claude Code a memory that survives `/clear`, new sessions, and other
projects.

## Prerequisites

A running daemon and an API key ([docs/mcp.md](../mcp.md#setup)):

```bash
recuerdos-ai serve &                      # leave running
recuerdos-ai user add alex
recuerdos-ai key issue --user alex --scopes read,write
```

## Configure the MCP server

Add Recuerdos AI to `~/.claude.json` (all projects) or a project's
`.mcp.json` (that project, shareable with your team):

```json
{
  "mcpServers": {
    "recuerdos-ai": {
      "command": "recuerdos-ai",
      "args": ["mcp", "--client", "claude-code"],
      "env": {
        "RECUERDOS_AI_API_KEY": "ra_live_…"
      }
    }
  }
}
```

`--client claude-code` is recorded as the source of every memory this
client saves, so `GET /v1/audit` can tell an editor's writes from a
script's.

If you run the daemon somewhere other than `localhost:7070`, add
`"RECUERDOS_AI_URL": "http://…"` to `env`.

### If the daemon runs on another host (HTTP)

The block above spawns a local `recuerdos-ai` process, so it needs the
binary on the same machine as Claude Code. If your daemon lives on a
server, connect straight to its `/mcp` endpoint instead — no local binary,
no shim:

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

The daemon must have the HTTP transport enabled (it is on by default;
`[server].mcp.http`) and be reachable at that URL. Put it behind TLS if it
leaves your machine — the bearer token is the only thing guarding it. See
[mcp.md](../mcp.md#setup) for the transport details.

Restart Claude Code, then check it connected:

```
/mcp
```

You should see `recuerdos-ai` with four tools (`memory_save`,
`memory_recall`, `memory_forget`, `session_distill`). If not, see
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
            "command": "curl -sf -H \"Authorization: Bearer $RECUERDOS_AI_API_KEY\" http://127.0.0.1:7070/v1/profile || true"
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
posts the summary to Recuerdos AI:

```json
{
  "hooks": {
    "PreCompact": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "jq -Rs '{content: ., client: \"claude-code\"}' | curl -sf -X POST -H \"Authorization: Bearer $RECUERDOS_AI_API_KEY\" -H 'Content-Type: application/json' -d @- http://127.0.0.1:7070/v1/sessions/distill >/dev/null || true"
          }
        ]
      }
    ]
  }
}
```

This posts to `/v1/sessions/distill`, which asks the extraction pipeline
a stricter question than an ordinary save does: *what is still true after
this session ends?* The two or three durable facts get pulled out — a
convention that was established, a root cause worth not rediscovering —
and everything about the task itself is discarded.

The request is synchronous, because a hook has nowhere to put a job id
and no next turn in which to poll. It is fast enough not to be felt, and
the `|| true` means a slow or unreachable daemon never blocks compaction.

Most sessions distil to nothing, which is the correct outcome and not a
sign the hook is broken.

**It only works with a provider configured.** With
`[understanding].provider = "none"` the endpoint returns `400` rather
than storing your whole transcript as one coarse memory — that memory
would be unrecallable and would crowd out better ones on every future
recall. In that mode, prefer letting the model call `memory_save`
deliberately: one good memory beats a transcript.

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

Delete the `recuerdos-ai` entry from your MCP config. Your memories stay
in the daemon's database; export them first with
`curl .../v1/memories/export` if you want them elsewhere.
