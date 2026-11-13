# Fields and replies of a memory save

The detail behind SKILL.md §3–4. The tool descriptions carry most of it; read this when
one of them isn't enough.

## Contents

- Which kind
- Topic, origin and the optional fields
- Every reply a save can get

## Which kind

| The user says… | kind | applies_to |
|---|---|---|
| "I always…", "I prefer…" (about themselves, everywhere) | `preference` / `rule` | `me` |
| "In this repo we…", rules in CLAUDE.md | `convention` / `rule` | `project` |
| "We chose X over Y because…" | `decision` | `project` (or `general`) |
| "The API is written in Rust", "staging resets nightly" | `fact` | `project` |
| "The NAS backups are in /mnt/tank" | `reference` / `fact` | `general` |
| "After that outage we learned…" | `learning` / `experience` | wherever it applies |
| "To release: tag, wait for CI, …" | `procedure` | `project` or `me` |
| "My goal is…" | `goal` | `me` or `project` |
| "Dentist on 14 December" | `plan` + `valid_until` | `general` |

The server's rules: a `preference` is only ever `me`; a `convention` is only ever
`project`; a `decision` is never `me`; a `goal` has no end date and stays open until the
user closes it; a `plan` needs `valid_until`.

## Topic, origin and the optional fields

- **`topic`**: `coding` for writing software; `tech` for machines, servers and services
  outside a codebase; then `work`, `people`, `family`, `health`, `lifestyle`, `home`,
  `finance`, `travel`, `food`, or one of the user's own topics.
- **`origin`**: `user_said` (they said it now), `user_confirmed` (you proposed, they
  agreed), `document` (read from CLAUDE.md, a README…), `inferred` (your own
  observation; refused for `me`).
- Optional: `subtopic` ("testing"), `tags`, `valid_until` (`YYYY-MM-DD`).
- Leaving `applies_to` out makes the server default to the current project (or
  `general`) and ask you to confirm it with the user.

## Every reply a save can get

| Reply | What it means | What to do |
|---|---|---|
| `conflict` | It contradicts (or, without a model, nearly repeats) a stored memory, named by id | Show the user both and ask which holds. Changed their mind → save with `replaces: "<id>"`. The stored one was wrong → `memory_update` it. Both true → save with `coexists_with: ["<id>"]`. |
| `already_known` | The store already says this | Nothing to do. |
| `needs_confirmation` | An inferred memory about the user, or closing a goal on your own | Ask; on yes, retry with `origin: "user_confirmed"`. |
| `scope_contradiction` | The text is tied to one project ("the codebase", "este repo") but filed as `me`/`general` | Use `target: "project"`, or reword if it really holds everywhere. |
| `kind_target_mismatch` | e.g. a `preference` for a project | Use what the message suggests (a project's way of doing things is a `convention`). |
| `unknown_project` | The folder's project is not registered yet | Ask whether it is new; on yes retry with `applies_to.new_project: true`. If the message asks "Is it a rename of X?", ask, and on yes call `project_rename` (X → this folder's name), then retry. |
| `no_project` | `target: project` with no project known | Ask which project, and name it in `applies_to.project_name`. |
| `secret` | A credential in the text | Save how to reach the thing instead. |
| `missing_field` / `invalid_value` | A field is missing or malformed | Fix what the message names. |
