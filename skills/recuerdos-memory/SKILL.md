---
name: recuerdos-memory
description: How and when to use the Recuerdos AI long-term memory (the `memory_*`, `session_distill` and `project_*` MCP tools, and the `memory://profile` resource). Use it at the start of every session in a project that has these tools, before assuming how the user works (their tooling, conventions, architecture decisions, preferences), whenever the user states something that stays true after this conversation (a preference, a rule, "in this repo we…", a decision and its reason, a goal, a dated plan) — even in passing, in the middle of a task — when they correct or change their mind about something remembered, when a memory save is refused, and before the session ends or the context is compacted. Use it even when the user does not mention memory.
---

# Recuerdos AI memory

The user keeps one long-term memory shared by every agent and session. Used well, they
never repeat themselves. Used badly, it fills with task chatter and files one project's
rules as the user's personal taste. The tool descriptions say what each field means and
the server explains every refusal; this skill covers what they can't: **when** to read,
**when** to save, and how to read what comes back.

## 1. Start of a session

1. **Read `memory://profile`** before doing work. It is the briefing: the user's own
   preferences, rules and open goals, this project's conventions, then general rules.
   Other projects are left out on purpose.
2. **Recall with the question you actually have** — `"how are use cases named here"`,
   `"how does the user like errors handled"` — not the project's name or a keyword list.
   Recall already knows the current project (from the folder) and ranks it with the
   user's own memories. Ask a second, broader question when the task touches something
   the first answer didn't cover (error handling, testing, naming, tooling).
3. **Follow what applies** without asking, and say so in a few words when it shapes a
   choice ("Using Vitest — you prefer it over Jest").

## 2. Reading recall results

Each line starts with `[kind · topic · who]`:

- **`me`** — the user, everywhere. A `preference` or `rule` is an instruction.
- **This project** — binding here, and it **overrides** a personal preference here.
- **Another project** (any other name) — how *that* project does it. **Never apply it
  here as the user's preference.** Offer it as an example ("fern returns `Result`s —
  want the same here?") and, if the choice matters, ask or leave it marked undecided.
- **`general`** — the user's machines, services, places, accounts.
- An empty result means nothing is stored, not that the user has no opinion.

## 3. When to save

Save **the moment** the user states something that will still be true after this
conversation: a preference ("I always…"), a project rule or convention ("in this repo
we…"), a decision and its reason, a fact about the project or their setup, a goal, a
dated plan. **That includes things said in passing**: "btw, we use date-fns, moment is
banned" while asking for a helper is a decision of this project — save it, do the task,
mention the save in one line. Several at once (importing a rules file) go in one
`memory_save_batch`.

Don't save: task state (what you're editing, what a test printed), what the repo already
says, secrets (save how to reach a thing, never the key), or your own guesses about the
user — ask them first, and save on a yes with `origin: "user_confirmed"`.

**Who it applies to is the field that matters most.** "my", "I always", "in every
project" → `me`; "here", "this repo", "we", anything read from the project's files →
`project` (the current one is filled in for you); machines, places, appointments →
`general`. **If you can't tell, ask.** Write `content` as one standalone sentence with
its subject ("The user prefers…", "Quill's API…"), one fact per memory. The full field
guide is in [references/fields.md](references/fields.md) if a tool description isn't
enough.

**Repeat the server's reply in one line** ("Saved as a coding rule of quill.") so the
user can correct it on the spot.

## 4. When a save is refused

Follow the message; it says what to do. **Never reword a memory to get past a refusal.**
The ones that need the user:

- **`conflict`** — it contradicts a stored memory (named by id). Show both, ask which
  holds: changed their mind → save with `replaces`; the old one was wrong →
  `memory_update` it; both true → save with `coexists_with`.
- **`unknown_project`** — ask whether this is a new project (then retry with
  `new_project: true`) or, if the message asks, a rename (then `project_rename`, retry).
- **`needs_confirmation`** — ask; on yes, retry with `origin: "user_confirmed"`.
- **`no_project`** — no project is known (an HTTP connection without project headers).
  Name it in `applies_to.project_name`, from the folder's repository name or by asking.

Every code is listed in [references/fields.md](references/fields.md).

## 5. Changing what is stored

- A memory is **wrong** (typo, scope, kind) → `memory_update` with only what changes.
- The user **changed their mind** → a new save with `replaces`, so history stays true.
- **Goals** close only on the user's word (`goal_status` + `origin: "user_said"`); if a
  goal merely looks done, ask.
- **Forget** only when asked: `memory_forget` returns candidates; delete only the ids
  the user confirms.
- `project_rename` / `project_merge` only with the user's agreement.

## 6. End of a session

When the session ends or the context is about to be compacted, call `session_distill`
with an honest summary. It keeps what outlives the session and drops the task;
returning nothing is a normal result. If the server says it has no model, skip it.

## 7. Manners

Mention memory when it changes what you do and when you save; don't narrate every
recall. Don't save the same thing twice or re-save what the profile shows. "Remember…"
and "forget…" are explicit requests: do them and confirm. If the memory is unreachable,
say so once and carry on.

Not a coding agent in a project folder (Hermes, Open WebUI, scripts, plain HTTP)? Read
[references/other-harnesses.md](references/other-harnesses.md).
