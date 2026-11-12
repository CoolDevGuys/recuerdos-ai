# The memory model: what a memory is, and who it is true for

Every memory says **what sort of statement it is**, **what it is about**, **who or
what it is true for**, and **where it came from**. That structure is what lets recall
answer "what are *my* coding preferences" without handing back one project's rules,
or offer one project's stack when you ask for your favourite framework.

You state the structure when you save a memory: in `memory_save` over MCP,
`POST /v1/memories:direct` over REST, `save_direct` in the Python SDK, and the
**Add memory** form in the web UI. The daemon checks it against a few rules, and
against what is already stored, and refuses what does not fit, saying why and how
to fix it. The design and its reasons
are in [ai-specs/memory-structure-plan.md](../ai-specs/memory-structure-plan.md).

## A save, field by field

```jsonc
{
  "content": "The user prefers mocks over real dependencies in unit tests",
  "kind": "preference",
  "topic": "coding",
  "applies_to": { "target": "me" },
  "origin": "user_said",
  "subtopic": "testing",          // optional
  "tags": ["testing", "mocks"],   // optional
  "valid_until": null             // required for a plan
}
```

The reply says how it was filed. An agent repeats it, so you can correct it at once:

> Saved as your personal coding preference (applies to you everywhere).

### `content` (required)

One standalone sentence that still makes sense months from now with no conversation
around it.

- Write the subject out: "The user…", "Quill's API…", not "it" or "we".
- One fact per memory: "prefers pnpm and Vitest" is two memories.
- **Never a secret.** Store *how* to reach something (host, user, which key file or
  vault entry), not the password, token or key. A save that looks like it contains
  one is refused.

| ✅ | ❌ |
|---|---|
| "The user prefers mocks over real dependencies in unit tests" | "use mocks" |
| "The homelab is reached through the bastion host as user `pi`" | "ssh pi@10.0.0.4 password hunter2" |

### `kind` (required): what sort of statement it is

| Kind | Ask yourself | Example | Applies to |
|---|---|---|---|
| `preference` | Is this what you pick when free to choose? An agent may deviate, with a reason. | "Prefers pnpm" | `me` |
| `rule` | Is this a hard line an agent must never cross? | "Never push to main" | any |
| `convention` | Is this how *one project* agreed to do things? | "Use cases are named by intent, no Service suffix" | `project` |
| `decision` | Was a choice made at a point in time, for a reason? | "Chose SQLite over Postgres for the smaller installer" | `project`, `general` |
| `fact` | Is this a durable truth about something or someone? | "Quill's frontend runs React 19" | any |
| `experience` | Is this something that happened? | "The March migration failed on index size" | any |
| `learning` | Is this the lesson drawn from it? | "Flaky tests usually hide a shared clock" | any |
| `procedure` | Is this a set of steps to follow again? | "Restore the NAS: stop the containers, then…" | any |
| `reference` | Is this a pointer to something else? | "Backups are in `/mnt/tank/backups`" | any |
| `goal` | Is this something you are working towards? It stays open until you say it is done. | "Run a half marathon in April" | `me`, `project` |
| `plan` | Is this a dated thing ahead? Needs `valid_until`. | "Dentist on 14 December at 10:00" | `general`, `project` |

A kind that cannot apply where you said is refused with the kind that would fit: a
project's way of doing things is a `convention`, not a `preference`.

### `topic` (required): what it is about

| Topic | For | Not for |
|---|---|---|
| `coding` | Writing software: languages, tools, style, testing, architecture | Running servers → `tech` |
| `tech` | Machines, servers, services, networks, accounts, configuration outside any codebase | Code → `coding` |
| `work` | Jobs, colleagues' roles, ways of working, meetings | |
| `family` | Relatives and the household | Friends, colleagues → `people` |
| `people` | Everyone else: friends, colleagues, contacts | |
| `health` | Body, sleep, medical, exercise as health | |
| `lifestyle` | Routines, hobbies, leisure, habits | Diet → `food` |
| `home` | The house, appliances, chores, maintenance | Home servers → `tech` |
| `finance` | Money, bills, subscriptions, taxes | |
| `travel` | Trips, transport, places | |
| `food` | Diet, cooking, restaurants | |

An unknown topic is refused with the list of known ones, so a typo cannot create one.

### `applies_to`: who or what it is true for

```jsonc
{ "target": "me" }                                  // you, everywhere
{ "target": "project", "project_name": "quill" }    // one project
{ "target": "general" }                             // your world, no project
```

| `target` | Means | Example |
|---|---|---|
| `me` | Who you are and how you like things, everywhere | "Prefers mocks in unit tests" |
| `project` | True of one project | "Quill forbids mocking the database" |
| `general` | True of your world, not tied to a project | "The NAS backups are in `/mnt/tank/backups`" |

How an agent chooses: "my", "I always", "personally", "in every project" → `me`.
"here", "this repo", "we", or a line read from a project's files → `project`.
Machines, files, services, places and appointments outside a project → `general`.
**When it is unclear, the agent should ask you.**

- **Left out,** it defaults to the current project when one is known, otherwise
  `general`, and **never** `me`: a wrong `me` would show one project's rule among your
  preferences everywhere. The reply says a default was used, so the agent confirms it.
- **`project_name`** is only for `target: "project"`, and is a lowercase name like
  `quill`. Inside a project folder the local MCP server detects it (below), so an
  agent rarely types it.
- **`new_project: true`** creates a project the daemon does not know yet. An agent
  sets it only after you confirm the project is new; without it, an unknown name is
  refused with the projects you have, so one project cannot split into several
  spellings. The web UI sets it for you: a name you type there is one you mean.

### `origin` (required): where it came from

| Value | When | The daemon… |
|---|---|---|
| `user_said` | You stated it | saves it as given |
| `user_confirmed` | The agent proposed it and you said yes | saves it as given |
| `inferred` | The agent noticed a pattern itself | **refuses it for `me`**: the agent must ask you first |
| `document` | Read from a file (CLAUDE.md, CONTRIBUTING, a README) | files it under the project, and turns a `preference` into a `convention` |

The memory's confidence comes from its origin.

### Optional fields

| Field | Purpose | Example |
|---|---|---|
| `subtopic` | A finer label within the topic | `testing`, `sleep` |
| `tags` | Keywords for filtering | `["pnpm", "tooling"]` |
| `valid_until` | When it stops being true; it leaves recall after this date. Required for a `plan`; not allowed on a `goal`. Over MCP a date (`2026-12-14`) means through the end of that day, UTC. | `"2026-12-14T23:59:59Z"` |
| `category` | The old category to also file it under, while recall filters and the map still read categories. Derived from kind and topic when left out. | `"health.sleep"` |
| `replaces` | The id of a stored memory this one replaces, when the user changed their mind. See [Contradictions](#contradictions-amend-replace-or-keep-both). | `"019f7c5a-…"` |
| `coexists_with` | Ids of stored memories the user said this one does not replace: both hold. | `["019f7c5a-…"]` |

## Projects

A memory that applies to a project stores the project's id, not its name.

- **Detected by the local MCP server.** `recuerdos-ai mcp` runs inside the folder your
  harness works in and sends that folder's project with every request:
  1. `RECUERDOS_AI_PROJECT`, when set (for example in `.mcp.json`'s `env`);
  2. the repository's `origin` remote, normalised (`github.com/acme/quill`), named
     after its last segment, so every clone and worktree is the same project;
  3. the repository's folder name, when it has no remote.

  Outside a git repository there is no project.
- **Over HTTP MCP and REST** there is no folder: name the project in
  `applies_to.project_name`, or send the `X-Recuerdos-Project` header (and optionally
  `X-Recuerdos-Project-Remote`).
- **The first save** to a project the daemon does not know is refused until you
  confirm it is new; then the project is created and its remote remembered.

## Contradictions: amend, replace, or keep both

Before a memory is stored, it is compared with the nearest stored memories **for
the same scope**: your own memories for `me`, one project's for that project, and
`general` for `general`. Kind and topic need not match; a `rule` can contradict a
`preference`. Memories that were replaced, have expired, or are closed goals are
not compared.

| What the check finds | What happens |
|---|---|
| A stored memory says the opposite | Refused as `conflict`, naming it and its id |
| A stored memory already says this | Nothing is stored; the reply is `Already known: …` (REST: `200`, `status: "already_known"`) |
| A project memory contradicts one of your own | Stored. A project may make an exception, and the reply says it overrides your preference there |
| Another project's memory says the opposite | Stored: projects do not contradict each other |

**With a language model configured** (`[understanding].provider`), the model judges
each neighbour as *contradicts*, *same* or *unrelated*. In the same call it also
checks whether text filed as `me` or `general` is really about one project, which
catches what the phrase list misses ("here the convention is…"). That is one
model call per save at most, and none for a project memory with nothing near it.

**Without a model**, similarity alone cannot tell "prefers tabs" from "prefers
spaces" apart from "prefers Rust" and "prefers Go", so the check only stops a
save that nearly repeats a stored memory, and says it could not judge
(`judged: false`). Real contradiction checking needs a model.

When a save is refused as a conflict, ask the user which holds:

| The user says | What to do | Result |
|---|---|---|
| "I've changed my mind" | Save again with `replaces: "<id>"` | The new memory is stored; the old one is superseded: out of recall, kept in history |
| "The old one was wrong" | Amend it: `memory_update` over MCP, `PATCH /v1/memories/{id}` over REST | Same memory, corrected; the audit trail keeps the old text |
| "Both hold, in different cases" | Save again with `coexists_with: ["<id>"]`, or reword it so the case is clear | Two memories |
| "Never mind" | Nothing | Nothing changes |

`replaces` must name your own active memory with the same `applies_to`. Moving a
memory to another scope is an amendment (change its `applies_to`), not a
replacement.

## Amending a memory

`memory_update` (MCP) and `PATCH /v1/memories/{id}` (REST) change a memory in
place: `content`, `kind`, `topic`, `applies_to`, `subtopic`, `tags`,
`valid_until`, `goal_status`, `origin`. The same rules as a save apply to what it
becomes, so it cannot be amended into a secret or a kind its target does not
allow. An amendment is not checked for conflicts, because it is how one is
resolved.

### Closing a goal

A goal stays `active` until **the user** says it was reached or given up:

```jsonc
{ "goal_status": "achieved", "origin": "user_confirmed" }   // or "abandoned"; "user_said"
```

An agent that only noticed it ("finished the race in 1:58") is refused with
`needs_confirmation`, and should ask: *"It looks like you ran the half marathon.
Shall I mark that goal as achieved?"* A closed goal leaves recall and the profile,
and stays in history. `"active"` reopens it.

## Memories the AI pipeline extracts

Raw text sent to `POST /v1/memories` and sessions sent to `session_distill` are
split into memories by the model, and each one gets the same structure, under the
same rules, as a save. A job cannot ask anyone anything, so where a save would be
refused it files the memory somewhere safe instead:

- what the model only **inferred** about you is never stored as yours. It comes
  back as a **suggestion** for the agent to ask you about;
- a credential is dropped;
- a project's rule with no known project is filed as `general`, and one that
  names a kind its target does not allow is moved to one that fits. Both are
  marked `needs_review`.

Send the `X-Recuerdos-Project` header to say which project the text is about. The
local MCP server does this for you. Only a project you already have is used.

## When a save is refused

A refusal is HTTP `422` with a code, and over MCP a tool result beginning
`Not saved (code):` (`Not updated (code):` for an amendment). The message says what
to change. Usually it means asking you.

| Code | Why | What to do |
|---|---|---|
| `missing_field` | `kind`, `topic` or `origin` is missing, a plan has no `valid_until`, or the body uses the old shape | Fill it in |
| `invalid_value` | A value the field does not take: an unknown kind, a past `valid_until`, a project name with spaces, a retired field (`subcategory` → `subtopic`, `expires_at` → `valid_until`, `confidence`) | Use one the message lists |
| `unknown_topic` | Not a built-in topic or one of yours | Pick one the message lists |
| `kind_target_mismatch` | The kind cannot apply there (a `preference` for a project) | Use the kind the message suggests, or the right target |
| `needs_confirmation` | An inferred memory about you | Ask, then save with `origin: "user_confirmed"` |
| `scope_contradiction` | The text ties itself to one project ("the codebase", "este proyecto") but would apply to you or in general | Use `target: "project"`, or reword it if it really holds everywhere |
| `no_project` | `target: "project"` with no project known | Ask which project, and name it |
| `unknown_project` | A project the daemon does not know | Ask whether it is new (`new_project: true`) or a known one under another name |
| `secret` | It looks like it contains a password, token or key | Store how to reach it, not the secret |
| `conflict` | It contradicts (or, without a model, nearly repeats) a stored memory for the same scope; `details.conflicts` names them | Ask the user, then use `replaces`, `memory_update` or `coexists_with` ([above](#contradictions-amend-replace-or-keep-both)) |

The text checks (`scope_contradiction`, `secret`) are tuned to refuse only on a clear
signal: on their labelled test sets, in English and Spanish, they refuse nothing they
should keep.

## Memories saved before structured memories

Every memory stored before this release was given a structure on the daemon's first
start, derived from its category. What could not be placed is marked
`needs_review: true`: every coding preference, since it may really be one project's
convention, and every project fact and decision, since the project was not on record.
A review screen in the web UI is planned; until then the field is visible in the API
and the JSON export.
