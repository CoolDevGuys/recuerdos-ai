# The `recuerdos-memory` skill

The MCP server gives an agent the tools, and its tool descriptions and refusals say what
each field means. What they can't say is **when** to use them: read the profile before
the first change, save a rule the moment the user states it (even in passing, in the
middle of a task), file a project's rule under the project and not as the user's taste,
and treat another project's convention as an example rather than an instruction.
[`skills/recuerdos-memory/`](../../skills/recuerdos-memory/SKILL.md) is an
[Agent Skill](https://agentskills.io) that teaches that.

**Is it worth it?** It was tested on five coding sessions (start of a session, a project
rule and a personal preference stated together, a change of mind, a decision mentioned
in passing, another project's convention), each with the skill and with a plain
"search memory at session start" note:

| Model | With the skill | With the plain note |
|---|---|---|
| Opus 5.5 | 100% | 100% |
| Sonnet 5.5 | 100% | 83% |
| Haiku 5.5 | 100% | 77% |

Without it, the smaller models did the task but saved neither the project's rule nor the
user's preference, and didn't save a decision mentioned in passing. A strong model
mostly gets there from the server's own guidance; the skill makes the cheaper ones
behave the same. It costs roughly 10–80k tokens per session. The cases are in
[`skills/recuerdos-memory/evals/`](../../skills/recuerdos-memory/evals/README.md).

## Install it

The skill is the whole `skills/recuerdos-memory/` folder: `SKILL.md` and its
`references/`, which the agent opens when it needs them. The `evals/` folder is for
testing the skill and is never read by an agent; leave it out of a copy if you like.

**Claude Code**, for every project on this machine:

```bash
ln -s ~/Dev/recuerdos-ai/skills/recuerdos-memory ~/.claude/skills/recuerdos-memory
```

(with your checkout's path), or copy the folder there on a machine without the
repository. For one project only, put it in that project's `.claude/skills/`.

**Other harnesses that support Agent Skills** (opencode, Hermes, …): put the folder
where that harness looks for skills; see its documentation.

**Harnesses without skill support** (Open WebUI, a plain system prompt): paste the
system-prompt version from
[`references/other-harnesses.md`](../../skills/recuerdos-memory/references/other-harnesses.md#a-system-prompt-for-harnesses-without-skill-support).

## Make sure it loads

A skill loads when the model decides it is relevant, and a coding task rarely looks like
a memory task: in testing the skill went unused until one line asked for it. Put this in
the project's `AGENTS.md` (or `CLAUDE.md`), replacing any older memory instructions:

```markdown
# Memory

This project uses the Recuerdos AI memory. At the start of every session, before
other work, load the `recuerdos-memory` skill and follow it.

If that skill isn't available, read `memory://profile`, recall with the question you
actually have before assuming how the user works, and save what stays true after
this conversation (preferences, project rules, decisions) the moment it's said.
```

The first paragraph loaded the skill in every tested session. The second is for agents
that read `AGENTS.md` but cannot load skills. Avoid the older advice to "search memory
for the project name": recall already knows the project it is in (through the stdio
shim, or HTTP's project headers — see [mcp.md](../mcp.md#which-transport-stdio-or-http)),
so the name adds nothing, and a real question matches far better.

With Claude Code's [`SessionStart` profile hook](claude-code.md#pull-the-profile-at-session-start)
the profile is already in context; the skill then goes straight to recalling for the
task.
