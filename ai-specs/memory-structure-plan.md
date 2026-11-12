# Recuerdos AI — Structured memories: what a memory is, and where it applies

Opened **2026-11-12**. Owner request, after testing the web UI: *asking for "my coding preferences"
returns my real preferences mixed with rules and decisions I applied in specific projects, which are
not my standards. Is there a way to improve how memories are stored — a better structure, format or
classification — so that recall can tell them apart? I would like to define as much structure as
possible when memories are saved, instead of relying on the harness to get it right.*

Status: **S0 implemented** (2026-11-12, branch `feat/memory-structure`); S1–S5 to do. The shape,
the rules and the examples in §3–§7 were settled with the owner in conversation on 2026-11-11/12
(§1). §11 records how the last open questions were resolved; none remain. §12 records how S0 went.

---

## 1. Decisions taken

| # | Question | Decision |
|---|---|---|
| D1 | How to fix it | A first-class **"applies to"** on every memory (who or what it is true for), plus a sharper **kind** taxonomy (a project's rule is not the user's preference). |
| D2 | `applies_to` values | **`me`** (who the user is, everywhere), **`project`** (one named project), **`general`** (the user's world, not tied to a project: machines, files, services, appointments). |
| D3 | Shape of `applies_to` | An object: `{ "target": "me" \| "project" \| "general", "project_name": "…" }`. `project_name` only with `target: "project"`. |
| D4 | Default when `applies_to` is missing | `project` when a project is known, else `general`. **Never `me`.** The reply says a default was used, and the agent is told to confirm it with the user. |
| D5 | Preferences the agent inferred | **Rejected for `me`.** The agent must ask the user, then save with `origin: "user_confirmed"`. |
| D6 | Contradictions | **Rejected with status `conflict`**, naming the stored memory it contradicts. The user then **amends** the old one (`memory_update`), **replaces** it (`memory_save` with `replaces`), or keeps both. |
| D7 | Kinds | `preference`, `rule`, `convention`, `decision`, `fact`, `experience`, `learning`, `procedure`, `reference`, `goal`, `plan` (§3.2). `decision` cannot apply to `me`. |
| D8 | Time-bound memories | `plan` requires **`valid_until`**; any memory may carry it. Expired memories leave recall. |
| D9 | Secrets | **Refused** on save and edit: store how to reach something, never the credential. |
| D10 | Project identity | From the **git remote** (`github.com/acme/quill` → `quill`), falling back to the folder name. Projects can be **renamed** (and merged) from MCP and from the web UI. |
| D11 | Topics | `coding`, `tech`, `work`, `family`, `people`, `health`, `lifestyle`, `home`, `finance`, `travel`, `food`, plus the user's own (§3.3). |
| D12 | Compatibility | The structured shape is **required** from the release that ships it. Old `category`-only saves are refused with a message naming the new fields. It is a breaking API change, so the release is **`v0.6.0`**. |
| D13 | Map | **Group by topic, colour by kind.** |
| D14 | Profile digest | Includes **`general` rules and procedures**, after `me` and the current project, within the digest's size budget. |
| D15 | Goals | A goal does **not expire by time**. It stays active until the **user confirms** it was achieved (or abandoned); an agent may only propose that. |
| D16 | The scope-contradiction check (R7) | Best accuracy for the cost: a phrase list in **English and Spanish** always; with a model configured, the model judges it **in the same call as the conflict check**, so it adds no extra request. |

## 2. The problem

A memory today has a `category` (`preference.coding`, `decision`, `fact.project`, …), an optional
`subcategory`, tags and a source (`client`, `session_id`). Nothing records **where it applies**.

- **Kind and scope are fused into one label.** "The user prefers mocks in tests" and "this repo
  forbids mocking the database" are both `preference.coding`. Recall cannot tell the user's standard
  from one project's rule, because the store never heard the difference.
- **The harness decides the label, and nothing checks it.** A caller that names a category gets it
  stored as sent (`intent_first_ingestor.rs`). One import of a project's rules file therefore files
  every "must" and "always" as the user's preference.
- **The classifier is never asked whose rule it is.** The extraction prompt reads "always / never" as
  a standing preference.
- **Recall cannot favour the current project.** A question asked inside one project gets another
  project's conventions as answers.

On the owner's store, 14 of the top 15 answers to "what are my coding preferences" were saved on a
single day from one project's rules file, all as `preference.coding`. The store holds no memory that
answers "which frontend framework do I prefer", and recall offered one project's stack instead.

**What we can and cannot control.** The harness decides whether to save and how to word it. We
control the save tool's **arguments** (required fields, fixed values), its **descriptions** (how each
field is chosen), **server-side rules** (reject with a reason and a fix; agents act on these well),
and the **reply** (how it was filed, which the agent repeats, so the user can correct it at once).
This plan puts structure in all four.

## 3. The memory model

### 3.1 The save shape

```jsonc
memory_save {
  "content": "The user prefers mocks over real dependencies in unit tests",
  "kind": "preference",
  "topic": "coding",
  "applies_to": {
    "target": "me",               // me | project | general
    "project_name": null          // only with target "project"; the local MCP server fills it in
  },
  "origin": "user_said",          // user_said | user_confirmed | inferred | document
  "subtopic": "testing",          // optional
  "tags": ["testing", "mocks"],   // optional
  "valid_until": null,            // required for kind "plan"
  "replaces": null                // optional: id of the memory this one supersedes
}
```

`memory_save_batch` takes a list of these and reports per item. `memory_update(id, …)` (new to MCP)
takes the same fields, all optional, and sends only what changed. `POST /v1/memories:direct` takes the
same body.

**Category becomes derived.** `kind` + `topic` replace `category` (`preference` + `coding` reads as
"coding preference"); `subtopic` replaces `subcategory`. §8 maps existing data.

### 3.2 `kind`: what sort of statement it is

Each kind comes with the question an agent asks itself, and the targets it may apply to. Kinds and
targets constrain each other on purpose: an illegal pair is rejected with the right alternative,
which is exactly the question that should have been asked.

| Kind | Ask yourself | Example | Allowed targets |
|---|---|---|---|
| `preference` | Is this what the user picks when free to choose? Agents may deviate, with a reason. | "Prefers pnpm" | `me` |
| `rule` | Is this a hard line an agent must never cross? | "Never push to main" | any |
| `convention` | Is this how *one project* agreed to do things? | "Use cases are named by intent, no Service suffix" | `project` |
| `decision` | Was a choice made at a point in time, for a reason? | "Chose SQLite over Postgres for the smaller installer" | `project`, `general` |
| `fact` | Is this a durable truth about something or someone? | "Quill's frontend runs React 19" | any |
| `experience` | Is this something that happened? | "The March migration failed on index size" | any |
| `learning` | Is this the lesson drawn from it? | "Flaky tests usually hide a shared clock" | any |
| `procedure` | Is this a set of steps to follow again? | "Restore the NAS: stop the containers, then…" | any |
| `reference` | Is this a pointer to something else? | "Backups are in `/mnt/tank/backups`" | any |
| `goal` | Is this something the user is working towards? Stays active until the user confirms it is done (§3.8). | "Run a half marathon in April" | `me`, `project` |
| `plan` | Is this a dated thing ahead? (`valid_until` required) | "Dentist on 14 December at 10:00" | `general`, `project` |

### 3.3 `topic`: what it is about

| Topic | Use for | Not for |
|---|---|---|
| `coding` | Writing software: languages, tools, style, testing, architecture | Running servers → `tech` |
| `tech` | Machines, servers, services, networks, accounts, configuration outside any codebase | Code → `coding` |
| `work` | Jobs, colleagues' roles, ways of working, meetings | |
| `family` | Relatives and the household | Friends, colleagues → `people` |
| `people` | Everyone else: friends, colleagues, contacts | |
| `health` | Body, sleep, medical, exercise as health | |
| `lifestyle` | Routines, hobbies, leisure, habits | Diet → `food`; medical → `health` |
| `home` | The house, appliances, chores, maintenance | Home servers → `tech` |
| `finance` | Money, bills, subscriptions, taxes | |
| `travel` | Trips, transport, places | |
| `food` | Diet, cooking, restaurants | |

Users add their own topics in the web UI (they replace the per-user categories of web UI P3). An
unknown topic is **rejected** with the list of known ones, so a typo cannot create a new topic.

### 3.4 `applies_to`: who or what it is true for

| `target` | Means | Example |
|---|---|---|
| `me` | Who the user is and how they like things, everywhere | "Prefers mocks in unit tests" |
| `project` | True of one project, named by `project_name` | "Quill forbids mocking the database" |
| `general` | True of the user's world, not tied to a project | "The NAS backups are in `/mnt/tank/backups`" |

How agents choose (this text goes into the tool's field description):

- "my", "I always", "personally", "in every project" → `me`.
- "here", "this repo", "we", or any line read from a project's files → `project`.
- Machines, files, services, places, accounts and appointments outside a project → `general`.
- **If unclear, ask the user.** When the agent omits `applies_to`, D4's default applies and the reply
  says so.

**`project_name`.** Required on the stored memory, optional in the call:

- The local MCP server (`recuerdos-ai mcp`, started by the harness inside the project folder)
  detects the project and attaches it to every request; the agent does not type it.
- Over HTTP MCP or REST (Hermes, claude.ai, scripts) there is no folder, so the caller must name it.
- A name the server does not know is rejected with the known ones: *"unknown project 'Quil'; known:
  quill, fern. If this is a new project, create it; if it is a rename, rename it."* One project cannot
  split into several spellings.

### 3.5 `origin`: where it came from

| Value | When | The server… |
|---|---|---|
| `user_said` | The user stated it in this conversation | saves it as given |
| `user_confirmed` | The agent proposed it and the user said yes | saves it as given |
| `inferred` | The agent noticed a pattern itself | **rejects it for `me`** (D5); allowed elsewhere |
| `document` | Read from a file (CLAUDE.md, CONTRIBUTING, a README) | forces `target: project`; turns `preference` into `convention` |

Confidence is derived from `origin` rather than asked for: agents' self-reported confidence was never
calibrated, and the origin is the honest signal.

### 3.6 Optional fields

| Field | Purpose | Example |
|---|---|---|
| `subtopic` | A finer label within the topic, for filtering | `testing`, `sleep` |
| `tags` | Keywords for filtering | `["pnpm", "tooling"]` |
| `valid_until` | When it stops being true; it leaves recall after this date. Required for `plan`; **not allowed on `goal`** (D15). | `"2026-12-14"` |
| `replaces` | The id of a memory this one supersedes (the user changed their mind). The old one stays in history. | `"mem_01J…"` |

### 3.7 `content`

One standalone sentence that still makes sense months later. The subject is written out ("The
user…", "Quill's API…"), one fact per memory, and **no secrets** (§4, R9).

### 3.8 Goals: closed by the user, not by time

A goal carries a **`goal_status`**: `active` (on save), `achieved` or `abandoned`.

- Closing it is a `memory_update(id, { "goal_status": "achieved", "origin": "user_confirmed" })`
  (or `user_said`). An `inferred` close is rejected (R12): the agent asks *"It looks like you ran the
  half marathon. Shall I mark that goal as achieved?"* and closes it only on a yes.
- A closed goal leaves recall and the profile but stays in history, and appears in the web UI under
  a **Closed** filter, so "what goals have I reached this year?" can still be answered.
- An achieved goal can be followed by an `experience` or `learning` ("Finished the half marathon in
  1:58"); the agent may suggest it.

## 4. Rules the server enforces

Every rule applies to every entry point (MCP, REST, the web UI, the pipeline), so no client can
bypass them. A rejection carries a stable `code`, one sentence of reason, and the fix.

| # | Rule | Code | Reply (abridged) |
|---|---|---|---|
| R1 | Required fields present and valid | `missing_field` / `invalid_value` | "`kind` is required: one of preference, rule, …" |
| R2 | Kind and target must be allowed together (§3.2) | `kind_target_mismatch` | "`preference` can only apply to `me`. A project's way of doing things is a `convention`." |
| R3 | `target: project` needs a known project | `unknown_project` / `no_project` | "No project is known here; name one, or save it as `general`." |
| R4 | `project_name` only with `target: project` | `invalid_value` | "`project_name` is only for target `project`." |
| R5 | Inferred preferences about the user need confirmation | `needs_confirmation` | "Ask the user whether this is their preference, then save with origin `user_confirmed`." |
| R6 | A memory read from a document belongs to its project | — (rewritten, not rejected) | "Saved as a convention of quill (from a document)." |
| R7 | The scope must not contradict the text | `scope_contradiction` | "This reads as a fact about one codebase ('the codebase'); use target `project`, or reword it if it holds everywhere." |
| R8 | No contradiction with a stored memory in the same scope (§5) | `conflict` | "It contradicts 'The user avoids mocks in unit tests' (id …). Ask the user…" |
| R9 | No secrets | `secret` | "This looks like a credential. Store how to reach it (host, user, which key file), not the secret." |
| R10 | `plan` needs `valid_until`, and it must be in the future | `missing_field` / `invalid_value` | "A plan needs `valid_until`: when does it stop being true?" |
| R11 | Unknown topic | `unknown_topic` | "Unknown topic 'helth'; known: coding, tech, …" |
| R12 | Only the user closes a goal; goals take no `valid_until` | `needs_confirmation` / `invalid_value` | "Ask the user whether the goal is achieved, then update it with origin `user_confirmed`." |

**R7** works in two layers (D16):

- **Always:** a short, tested list of phrases that bind a sentence to one codebase, in English and
  Spanish ("the codebase", "this repo", "this project", "we use" / "el código", "este repo", "este
  proyecto", "aquí usamos"). Microseconds, no model, and it fires only on clear signals.
- **With a model configured:** the model also judges whether the text fits its `applies_to`, as one
  more output of the call R8 already makes for conflicts. It catches what the list cannot (other
  languages, paraphrase) at no extra request. Without a model, the list alone applies.

Both layers are measured on a labelled set in both languages before shipping (§10).

**R9** matches known credential shapes: private-key headers, cloud and SaaS token prefixes
(`AKIA…`, `ghp_…`, `sk-…`, `xox…`), `password=` / `token:` pairs, and long high-entropy strings. It runs
on save, update and import. False positives are refused, not stored, and the reply says which part
looked like a secret.

### Replies on success

| Situation | Reply |
|---|---|
| Stored | "Saved as your personal coding preference (applies to you everywhere)." |
| Stored with a default (D4) | "Saved as general (no project here, and applies_to was not given). Confirm with the user." |
| Stored, overriding a `me` memory in a project | "Saved as a quill rule. It overrides your personal preference 'prefers mocks' in this project." |
| Replaced | "Saved; it replaces 'The user avoids mocks…', which is kept in history." |

## 5. Contradictions, amending and replacing

### 5.1 What a new memory is checked against

At save time the server looks for **similar memories with the same `applies_to`**: the same target
and, for `project`, the same project. It then judges whether any of them says the opposite.

| New memory | Compared against | If they disagree |
|---|---|---|
| `me` | other `me` memories | `conflict`: rejected |
| `project` quill | other quill memories | `conflict` |
| `general` | other `general` memories | `conflict` |
| `project` quill | the user's `me` memories | **not a conflict**: saved, and the reply notes the override (a project exception to a personal rule is normal) |
| anything | superseded, forgotten or expired memories | ignored |

Kind and topic need not match: a `rule` can contradict a `preference`.

**How it judges.** The nearest memories in the same scope (vector search, top 5, above a similarity
floor) are candidates. With a chat model configured, the model judges each pair as `contradicts`,
`same` or `unrelated`. The reconciler already makes this call for the pipeline; it is reused, not
rebuilt. `same` returns the existing "already known" status. Without a model, the closest candidates
above a higher floor are returned as **possible** conflicts with status `conflict` and
`judged: false`, and the agent asks the user.

### 5.2 What the user can do about a conflict

| The user says | Mechanism | Result |
|---|---|---|
| "The old one was wrong; it should say…" | **Amend**: `memory_update(id, …)`, new as an MCP tool (REST `PATCH` exists) | Same memory, corrected. The audit trail keeps the old text. |
| "I've changed my mind" | **Replace**: `memory_save(…, replaces: id)` | A new memory; the old one gets `superseded_by` (the field exists), leaves recall and stays in history. |
| "Both hold, in different cases" | Save with a scope or wording that tells them apart | Two memories, no conflict. |
| "Never mind" | Nothing | Nothing changes. |

`replaces` is checked: the id must exist, belong to the user, be active, and have the same
`applies_to`. Replacing across scopes is a rescope, which is an update.

## 6. Projects

A per-user **project registry** replaces free-text project names on memories.

- **Fields:** `id`, `name` (unique per user; what is shown and typed), `aliases` (detected
  identifiers: normalised remotes such as `github.com/acme/quill`, and folder names), `created_at`.
- **Detection** (local MCP server): normalised git remote, else the folder name. Worktrees and fresh
  clones share a remote, so they map to one project. The first save from an unknown identifier
  creates the project after a confirmation round-trip: *"New project 'quill' (github.com/acme/quill).
  Save it there?"*
- **Collision:** two remotes with the same repository name (`acme/quill`, `me/quill`) get distinct
  names (`quill`, `me-quill`), and the reply says so.
- **Rename:** a memory stores the project **id**, so a rename is one row and every memory follows.
  The old name stays as an alias, so older checkouts and harness configs still resolve.
  - MCP: `project_list`, `project_rename(name, new_name)`.
  - Web UI: a **Projects** tab: rename, see its memories, merge two projects.
  - REST: `GET /v1/projects`, `PATCH /v1/projects/{id}`, `POST /v1/projects/{id}/merge`.
- **A renamed repository:** the remote changes, so detection produces an unknown identifier. The
  reply offers the fix: *"unknown project 'quill-app' (github.com/acme/quill-app). Is it a rename of
  'quill'? Use project_rename, or create it."* Renaming adds the new remote as an alias.

## 7. Examples, with the expected result (acceptance tests)

Each row becomes an end-to-end test against a real daemon (MCP and REST), with a fake model where a
judgment is needed.

| # | Context | The user says / the agent sends | Expected |
|---|---|---|---|
| E1 | in quill | "Save as my personal preference: use mocks in tests" | stored: preference · coding · `me` · user_said. Reply: "personal coding preference (everywhere)". |
| E2 | in quill | "In this repo we never mock the database" | stored: rule · coding · `project` quill. Not a conflict with E1; the reply notes the override. |
| E3 | in quill | The agent sends an inferred preference "prefers small PRs" with target `me` | rejected `needs_confirmation`. After the user confirms: stored with `user_confirmed`. |
| E4 | in quill | "Import the rules from CLAUDE.md": one rule sent as `preference`, origin `document` | stored as convention · `project` quill (R6). |
| E5 | in quill | The agent sends "The codebase uses Python 3.12+" with target `me` | rejected `scope_contradiction`; the retry with `project` is stored. |
| E6 | anywhere | "Actually I now avoid mocks in unit tests" | rejected `conflict`, naming E1. The user says they changed their mind; saved with `replaces` = E1. E1 is superseded and kept in history. |
| E7 | anywhere | A typo in E1 ("mokcs") | `memory_update(E1, content)`: same id, the audit keeps the old text. |
| E8 | Hermes | "Remember the NAS backups are in /mnt/tank/backups" | stored: reference · tech · `general`. |
| E9 | Hermes | "Remember the server password is …" | rejected `secret`. The agent stores how to reach the server instead. |
| E10 | Hermes | "Dentist appointment on 14 December at 10:00" | stored: plan · health · `general` · `valid_until` 2026-12-14. Absent from recall from 2026-12-15. |
| E11 | Hermes | The agent sends `target: project` with no `project_name` | rejected `no_project`; the agent asks which project. |
| E12 | anywhere | The agent omits `applies_to` | stored with the D4 default; the reply flags it for confirmation. |
| E13 | anywhere | `preference` with `target: project` | rejected `kind_target_mismatch`, suggesting `convention`. |
| E14 | anywhere | A `category`-only save (old client) | rejected `missing_field`, naming `kind`, `topic`, `applies_to` and `origin`. |
| E15 | in quill-app (repo renamed) | Any project save | rejected `unknown_project`, offering a rename of 'quill'. After `project_rename`, quill's memories are found under the new name, and the old remote still resolves. |
| E16 | in fern | "What are my coding preferences?" | `me` preferences and rules, then fern's conventions, labelled. No quill conventions. |
| E17 | in fern | "How did quill handle command handlers?" | quill's convention is found: the question names the project, so the demotion of other projects (§9) does not hide it. |
| E18 | Hermes | "What's my favourite frontend framework?" | Only `me` memories; with none stored, an honest empty answer rather than one project's stack. |
| E19 | anywhere | "My goal is to run a half marathon in April" | stored: goal · health · `me` · `goal_status: active`. A `valid_until` on it is rejected. |
| E20 | anywhere | The agent reads "finished the race in 1:58" and closes E19 with origin `inferred` | rejected `needs_confirmation`. The agent asks; on yes, E19 becomes `achieved`, leaves recall and the profile, and shows under **Closed** in the UI. |
| E21 | in quill | The agent sends "Este proyecto usa Python 3.12" with target `me` | rejected `scope_contradiction` by the Spanish phrase list, with no model configured. |
| E22 | in quill, model on | The agent sends a paraphrase the list misses ("Here the convention is to …") with target `me` | rejected `scope_contradiction` by the model's judgment, made in the same call as the conflict check. |

## 8. Existing memories

V11 adds the new columns and the `projects` and `topics` tables, and maps every existing memory
deterministically. Anything it cannot place is marked **`needs_review`**.

| Old category | kind | topic | target |
|---|---|---|---|
| `preference.coding` | preference | coding | `me` (review: may be a convention) |
| `preference.personal` | preference | lifestyle | `me` |
| `decision` | decision | from tags, else `work` | `general` + `needs_review` (the project is unknown) |
| `fact.project` | fact | coding | `general` + `needs_review` |
| `fact.person` | fact | people | `general` |
| `experience` | experience | from tags | `general` |
| `skill` | procedure | from tags | `general` |
| `reference` | reference | from tags | `general` |
| a user category (`health.sleep`) | fact | `health` (topic), subtopic `sleep` | `general` + `needs_review` |

`subcategory` becomes `subtopic`. Every migrated memory gets origin `user_said` (the history cannot
tell), and `source.client` and the save date stay as they were.

The web UI gets a **Needs review** queue: filter, bulk-assign project, kind and target, and an
optional **Suggest** button that runs the classifier over the selection and pre-fills its answer for
the user to accept or change. Nothing is rewritten without the user's click.

## 9. Recall, profile, consolidation

- **Context.** A recall carries the caller's project when there is one: the local MCP server sends it,
  and REST and HTTP MCP accept it.
- **Scope-aware ranking.** `me`, `general` and the current project rank as they do today. **Other
  projects are demoted**, not hidden (E17), unless the query names the project. Without a project
  context (Hermes), project memories are demoted the same way.
- **Filters** on recall (REST, MCP, SDK): `kind`, `topic`, `target`, `project`. They replace the
  `categories` filter.
- **Expiry.** A memory past `valid_until` leaves recall, and the nightly job retires it.
- **Profile digest** (`memory://profile`), in this order and within its existing size budget:
  `me` preferences, rules and **active** goals; the current project's conventions and rules;
  `general` rules and procedures (D14). Today it is one digest per user; it becomes one per user
  and project context.
- **The output line** names the scope: `[preference · coding · me]`, `[convention · coding · quill]`.
- **Consolidation and reconciliation** only compare and merge memories with the same `applies_to`.
  Two projects' identical conventions are not merged; three or more are suggested to the user as a
  possible `me` rule (a review item, never automatic).
- **The pipeline** (raw text, session distillation, "let the AI classify it") outputs the same shape
  under the same rules. The caller supplies the project, and the extractor sets `origin`: `user_said`
  for what the user stated in the text, `inferred` for patterns. Inferred `me` items are returned as
  suggestions for the agent to ask about, never stored (D5).

## 10. Phases

All phases land on one branch and ship together as `v0.6.0` (D12). Each phase ends green and
reviewable.

| Phase | Scope | Exit |
|---|---|---|
| S0 — Model & migration | V11: new columns, `projects`, `topics`; the §8 mapping; domain types (`Kind`, `Topic`, `AppliesTo`, `Origin`) and the allowed-pairs table; reads expose the new fields. | The migration runs on a copy of the field store with every memory mapped or flagged; nothing lost. |
| S1 — Structured save | The new shape on REST direct save, `memory_save` and `memory_save_batch`; R1–R7 and R9–R11; replies; project detection in the stdio shim; the old shape refused. SDK, `docs/api.md`, `docs/mcp.md`, the integrations docs and the new `docs/memory-model.md`. | E1–E5, E8–E14, E19 and E21 pass. R7 (English and Spanish) and R9 precision measured on a labelled set (≥ 95% precision; a missed case is acceptable, a false rejection is not). |
| S2 — Conflicts, amend, replace | R8 and the model layer of R7, in one model call; the `conflict` status with or without a model; the `memory_update` MCP tool; `replaces`; closing goals (R12); reconciler and merger scoped by `applies_to`; the pipeline producing the shape. | E6, E7, E20 and E22 pass; the extractor set (raw text → expected fields) passes; the save p50 with a model rises by one model call at most. |
| S3 — Projects | The registry, aliases, collisions, rename and merge over MCP and REST. | E15 passes. |
| S4 — Recall & profile | Context, scope-aware ranking, the new filters, expiry, the per-project profile; eval cases for scope. | E16–E18 pass; the existing eval and field set do not drop. |
| S5 — Web UI | Editor fields with help text; list and map filters by kind, topic, target and project; the map **grouped by topic and coloured by kind** (D13); Projects and Topics tabs; the Needs review queue with Suggest; the Closed filter for goals; bulk edit of the new fields. | Playwright covers review, rename, closing a goal and a conflict shown to the user. The 11-kind palette passes the existing colour-vision test (contrast ≥ 3:1, ΔE ≥ 14 under each simulated vision type). |

## 11. Resolved questions

| Question | Resolution |
|---|---|
| Map grouping | Group by topic, colour by kind (D13). |
| `general` in the profile digest | Yes: `general` rules and procedures, after `me` and the current project (D14). |
| Do goals expire? | Not by time: when the user confirms they are achieved or abandoned (D15, §3.8). |
| The R7 phrase list is English | English and Spanish lists, plus the model's judgment in the call R8 already makes (D16). |

**Risk carried into S5.** Today's palette gives 8 built-in categories at ΔE ≥ 14 under every
simulated colour-vision type. Eleven kinds at that distance may not exist. If the test cannot be met,
related kinds share a hue at two lightness levels (`experience` / `learning`, `rule` / `convention`),
and the legend and tooltips name the kind, so colour is never the only cue.

## 12. Implementation notes

### S0 (2026-11-12)

- **The mapping lives in one place**: `memories::domain::legacy_structure`. V11 only adds nullable
  columns and the new tables; a startup backfill (`SqliteMemoryRepository::backfill_structure`) fills
  every row whose `kind` is NULL through that function, in one transaction, and a row it has not
  reached is read through the same function. SQL would have needed a second copy of the tag table
  and of the user-category splitting. Read-only openers skip the backfill.
- **`expires_at` is `valid_until`**; no new column. `subcategory` is the subtopic. `category` stays as
  the record of what was originally stated; S1 decides what writes it.
- **The backfill does not touch `updated_at`**: deriving a structure is not an edit, and the web UI's
  `If-Match` must not start refusing edits to memories the user never changed.
- **Until S1, a new memory's structure comes from its category**, and re-filing a memory re-derives
  it, since the category is still the only way a user states it.
- **The JSON export carries the structure**: it is the backup format and the web UI's list source.
- **On a copy of the field store** (frozen 2026-10-18, schema V9): V10 and V11 ran, the backfill filled
  225 of 225 rows (superseded and forgotten included), none is left without a structure, none holds
  an illegal kind × target pair, and 185 of the 222 active memories are flagged for review: 94 coding
  preferences, 65 project facts, the decisions, and the procedures, references and experiences no tag
  placed. The review queue (S5) will be long, and its **Suggest** pass is what keeps it usable.

