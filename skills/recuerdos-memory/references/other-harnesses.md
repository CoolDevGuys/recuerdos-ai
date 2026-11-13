# Using the memory outside a coding project

The rules in SKILL.md hold everywhere. This file covers what changes when you are not
a coding agent started inside a project folder: a chat assistant (Hermes, Open WebUI),
an HTTP MCP connection, or a script calling the REST API.

## Contents

- What changes without a project folder
- MCP tools and their REST equivalents
- A life assistant (Hermes and similar)
- Open WebUI
- A system prompt for harnesses without skill support

## What changes without a project folder

The stdio shim (`recuerdos-ai mcp`) detects the project from the folder it runs in.
Over HTTP MCP (`/mcp`), REST, or a shim started outside a git repository, **there is
no current project** — unless the client's config sends the `X-Recuerdos-Project`
(and optionally `X-Recuerdos-Project-Remote`) headers, which make an HTTP session
behave like the shim in that project. Without them:

- **Recall** ranks every project's memories below the user's own and general ones —
  the right default for a chat assistant. When the conversation *is* about one
  project, pass it: `memory_recall` takes `project: "quill"`; REST search takes
  `"project": "quill"` (or the `X-Recuerdos-Project` header).
- **Saving a project memory** needs the project named: `applies_to: {"target":
  "project", "project_name": "quill"}`. Without it the save is refused with
  `no_project`; ask the user which project.
- **The profile** has no project section. `GET /v1/profile?project=quill` adds one.
- Most of what a chat assistant hears is `me` (who the user is) or `general` (their
  world: places, people, appointments, accounts). Project memories are rare here.

## MCP tools and their REST equivalents

| MCP | REST | Notes |
|---|---|---|
| `memory://profile` | `GET /v1/profile` | markdown; `?project=` for a project section |
| `memory_recall` | `POST /v1/memories/search` | `query`, `limit`, filters `kinds`, `topics`, `targets`, `projects`; `project` = where the caller is; `scope: "all"` ranks every project alike |
| `memory_save` | `POST /v1/memories:direct` | same fields; `201` stored, `200` with `status: "already_known"`, `422` refused with `error.code` and `error.details` |
| `memory_save_batch` | one `:direct` per item | |
| `memory_update` | `PATCH /v1/memories/{id}` | only the fields that change; `goal_status` + `origin` closes a goal |
| `memory_forget` | `DELETE /v1/memories/{id}` | only on the user's request |
| `session_distill` | `POST /v1/sessions/distill` | needs a model on the server |
| `project_list` / `project_rename` / `project_merge` | `GET /v1/projects`, `PATCH /v1/projects/{name}`, `POST /v1/projects/{name}/merge` | with the user's agreement |
| — | `POST /v1/memories` | raw text through the understanding pipeline: it extracts, labels and supersedes for you. Good for a chat assistant that would rather not structure each memory; send what the user said, not your summary |

Every request carries `Authorization: Bearer <key>`. Full reference: `docs/api.md` in
the recuerdos-ai repository.

## A life assistant (Hermes and similar)

- **Topics** that carry most of a personal assistant's weight: `food`, `health`,
  `lifestyle` (routines, hobbies), `people` and `family`, `home`, `finance`, `travel`,
  and `tech` for the user's machines and accounts.
- **Filter recall** to keep a dinner booking free of the user's TypeScript opinions:
  `topics: ["food", "health"], targets: ["me"]`.
- **Plans** are dated: "Dentist on 14 December at 10:00" is a `plan`, `topic:
  "health"`, `applies_to: general`, `valid_until: "2026-12-14"`. It leaves recall after
  that date by itself.
- **Goals** ("run a half marathon in April") stay open until the user says they are
  done; never close one on your own.
- **People**: "Ana is my sister, allergic to nuts" → `fact`, `topic: "family"`,
  `applies_to: general`. It is a fact about Ana, not a preference of the user's.
- **Conflicts** matter more here than anywhere: "I'm vegetarian now" after a stored
  "loves steak" is a change of mind — ask, then save with `replaces`.

## Open WebUI

Open WebUI reaches MCP servers over HTTP, so point it at the daemon's streamable-HTTP
endpoint (`http://<daemon>:7070/mcp` with an `Authorization: Bearer` header), or
through an MCP-to-OpenAPI proxy if your version needs one. There is no project folder,
so follow "What changes without a project folder" above. Open WebUI has no skill
loader: paste the system prompt below into the model's or workspace's system prompt.

## A system prompt for harnesses without skill support

```text
You have a long-term memory shared with the user's other assistants
(tools: memory_recall, memory_save, memory_save_batch, memory_update,
memory_forget; resource memory://profile).

- At the start of a conversation, read memory://profile. Before assuming how
  the user lives or works, call memory_recall with the question you actually
  have.
- Recall lines are labelled [kind · topic · who]: "me" is the user, everywhere;
  "general" is their world; a project's memories belong to that project only.
- Save, the moment it is said, only what stays true after this conversation:
  preferences, rules, facts about the user and the people in their life,
  decisions and their reasons, goals, dated plans (with valid_until). Never
  save task chatter, and never a password, token or key.
- Each save states content (one standalone sentence with its subject), kind
  (preference, rule, fact, decision, experience, learning, procedure,
  reference, goal, plan), topic (food, health, lifestyle, people, family,
  home, finance, travel, tech, work, coding), applies_to ({"target": "me"} or
  {"target": "general"}; a project only when the user names one), and origin
  (user_said; user_confirmed after you asked; never save your own guesses
  about the user without asking).
- Tell the user in one line how each memory was filed.
- If a save is refused, follow the message. "conflict" names a stored memory:
  ask the user which holds, then save with replaces (they changed their mind),
  fix the old one with memory_update (it was wrong), or save with
  coexists_with (both hold). "needs_confirmation": ask, then use
  user_confirmed. Never reword to get past a refusal.
- Only the user closes a goal (memory_update with goal_status and origin
  user_said). Forget only when asked, and only the ids the user confirms.
```
