# Changelog

Format loosely follows [Keep a Changelog](https://keepachangelog.com/).
One entry per phase, backfilled as phases land.

## Unreleased

### Structured memories, phase S3: projects can be listed, renamed and merged

Phase S3 of [memory-structure-plan.md](../ai-specs/memory-structure-plan.md), part of
`v0.6.0`. Explained for users in [memory-model.md](memory-model.md#projects).

- **REST**: `GET /v1/projects`, `GET /v1/projects/{project}`, `PATCH
  /v1/projects/{project}` (rename) and `POST /v1/projects/{project}/merge`. A project
  is named by id, name or alias.
- **MCP**: `project_list`, `project_rename` and `project_merge`.
- **Python SDK**: `projects()`, `project()`, `rename_project()`, `merge_project()`,
  and the `Project` and `ProjectChange` models.
- **A rename is one row**: memories store the project's id and follow; the old name
  stays as an alias. Asked from inside a folder whose project is the new name, the
  folder's remote is learned too.
- **A repository renamed on its host** is no longer just "unknown project": the
  refusal asks whether it is a rename of the project with the same owner's remote,
  and `details.rename_of` names it (E15).
- **Collisions**: a second repository with the same name (`me/quill` next to
  `acme/quill`) is offered as its own project, `me-quill`, instead of being filed
  under the first. Before, its remote was silently added to `quill`.
- **Merging** moves every memory of one project into another, audited per memory;
  the merged project's name and remotes find the one that stays.
- `SaveNote::ProjectCreated` gained the name it would have had; the REST note is
  still `project_created`.

### Structured memories, phase S2: contradictions are caught at the door, and fixed or replaced on purpose

Phase S2 of [memory-structure-plan.md](../ai-specs/memory-structure-plan.md), part of
`v0.6.0`. Explained for users in
[memory-model.md](memory-model.md#contradictions-amend-replace-or-keep-both).

- **A save that contradicts a stored memory for the same scope is refused** as
  `conflict` (`422`), naming the stored memory and its id, with `details.conflicts`
  for clients. Compared only within the scope: your own memories for `me`, one
  project's for that project, `general` for `general`. A project memory that
  contradicts one of yours is stored, and the reply says it overrides your
  preference there. A save the store already knows stores nothing: `200` with
  `status: "already_known"`, `Already known: …` over MCP.
  - With a model configured, it judges the nearest five memories in one call, which
    also checks whether text filed as yours or as general is really one project's
    (the model half of the scope check; "here the convention is…" is now caught).
    One call per save at most, none for a project memory with nothing near it; an
    outage falls back to the model-free rule rather than losing the save.
  - Without a model, only a near repeat of a stored memory is stopped, with
    `judged: false`. Measured on the default embedder, contradicting and compatible
    pairs overlap in similarity, so the floor (0.75) sits above every compatible
    pair rather than catching every contradiction.
- **Three ways out**: `replaces` on a save (the user changed their mind: the old
  memory is superseded, out of recall, kept in history), an amendment (the old one
  was wrong), or `coexists_with` (both hold).
- **Amending**: the new MCP tool `memory_update`, and `PATCH /v1/memories/{id}`
  extended with `kind`, `topic`, `applies_to`, `goal_status` and `origin` (and
  `subtopic`, `valid_until` as the new names of `subcategory`, `expires_at`). What a
  memory becomes is held to the save rules (no secrets, allowed kinds, the scope
  phrase list); the reply carries a `message`. Re-filing a memory under another
  category no longer resets a structure someone stated.
- **Goals close only when the user says so**: `goal_status` `achieved` or
  `abandoned` with `origin` `user_said` / `user_confirmed`; `inferred` is refused
  with `needs_confirmation`. A closed goal leaves recall, the profile and
  consolidation, and stays in history.
- **The AI pipeline produces the structure**: extraction and session distillation
  ask the model for `kind`, `topic`, `applies_to` and `origin`, and file each memory
  under the same rules, safely where a save would refuse: an inference about you is
  returned in `suggestions` (outcome `needs_confirmation` when nothing else was
  stored), never stored as yours; a credential is dropped; a project memory with no
  known project is `general` and marked `needs_review`. `POST /v1/memories`,
  `POST /v1/memories/batch` and `POST /v1/sessions/distill` read the
  `X-Recuerdos-Project` headers (existing projects only).
- **Reconciliation and consolidation stay inside a scope**: a candidate is only
  reconciled against memories for the same scope, and duplicates are only merged
  with others for the same scope, keeping it.
- **MCP**: `memory_save` and `memory_save_batch` take `replaces` and
  `coexists_with`, and their descriptions say how to answer a conflict. Batch items
  the store already knew are listed apart from the stored ones.
- **Web UI**: a conflict in **Add memory** names the stored memory and offers
  **Replace it** or **Keep both**; saving something you already have says so.
- **Python SDK**: `save_direct(…, replaces=, coexists_with=)`, `SavedMemory.status`
  and `.replaced`; `update()` takes the structure and `goal_status` and returns an
  `UpdatedMemory` with `message`; `RejectedError.conflicts` and `.judged`, and a
  `422` is always a `RejectedError` (its `conflict` is not the `409` one).
- **An extraction structure set** (`eval/extraction-structure.toml`, 16 cases in
  English and Spanish) and a runner that needs a real model:
  `cargo test --bin recuerdos-ai extraction_structure_set -- --ignored`.

### Structured memories, phase S1: every save states what it is and who it is true for

Phase S1 of [memory-structure-plan.md](../ai-specs/memory-structure-plan.md). **A
breaking change to the save API**, shipping with the rest of the plan as `v0.6.0`.
The field reference is the new [docs/memory-model.md](memory-model.md).

- **`POST /v1/memories:direct`, `memory_save` and `memory_save_batch` take the
  structured shape**: `content`, `kind`, `topic`, `applies_to`
  (`{"target": "me" | "project" | "general", "project_name", "new_project"}`),
  `origin`, plus optional `subtopic`, `tags`, `valid_until` and `category`. The old
  shape is refused by name: `category` alone gets `missing_field`, and
  `subcategory`, `confidence` and `expires_at` get `invalid_value` naming their
  replacement. The reply carries `message` ("Saved as your personal coding
  preference (applies to you everywhere).") and `notes`.
- **The rules every entry point enforces** (`StructuredMemorySaver`), each refusal a
  `422` with a code and a fix:
  - a kind only applies where it can (a preference is yours; a convention is a
    project's) — `kind_target_mismatch`;
  - an inferred memory about you needs your confirmation — `needs_confirmation`;
  - a memory read from a document is filed under its project, and a preference in
    one becomes a convention;
  - text tied to one project ("the codebase", "este proyecto") cannot apply to you
    or in general — `scope_contradiction`;
  - no credentials — `secret`;
  - a project memory needs a known project, or one you confirm is new —
    `no_project`, `unknown_project`;
  - a plan needs `valid_until`; a goal takes none; unknown topics are refused.
  - With no `applies_to`, the default is the current project, else `general`, never
    `me`, and the reply asks for confirmation.
- **The two text checks are tuned for precision**, on labelled sets in English and
  Spanish: the scope check and the secret check each refuse none of the texts they
  should keep, and miss none of their test cases.
- **Projects.** A per-user registry (V11's `projects` and `project_aliases`).
  `recuerdos-ai mcp` detects the project of the folder it runs in —
  `RECUERDOS_AI_PROJECT`, else the git `origin` remote (`github.com/acme/quill` →
  `quill`, the same for every clone and worktree), else the repository's folder
  name; nothing outside a repository — and sends it as `X-Recuerdos-Project` /
  `X-Recuerdos-Project-Remote`. A remote is remembered as an alias.
- **MCP**: the save tools' descriptions carry the field guidance; a refusal is a tool
  result beginning `Not saved (code):`, which an agent reads and acts on.
- **Web UI**: **Add memory** asks what it is, what it is about and who it applies
  to, greys out what a kind cannot take, and shows a refusal without losing the
  text. The category is derived for a direct save; custom categories stay
  available to the AI path and the editor.
- **Python SDK**: `save_direct(content, *, kind, topic, origin, applies_to=…)`
  returns a `SavedMemory` with `message`; `RejectedError` (422) carries the rule's
  `code`; `Memory` gains the structure fields; `AppliesTo` is exported.
- Unchanged in S1: the editor, bulk edits and category management still work on
  categories (re-filing re-derived the structure until S2), and raw text through
  `POST /v1/memories` and session distillation still goes through the pipeline,
  which learns the shape in S2.

### Structured memories, phase S0: every memory has a kind, a topic and a scope

The first phase of [memory-structure-plan.md](../ai-specs/memory-structure-plan.md):
the model and the migration, with no change to how memories are saved yet.

- **Migration V11** adds `kind`, `topic`, `target` (`me` / `project` /
  `general`), `project_id`, `origin`, `goal_status` and `needs_review` to
  memories, and the `projects`, `project_aliases` and `topics` tables.
  Nothing is dropped: `category` and `subcategory` stay.
- **On first start, every existing memory gets a structure** derived from its
  category (plan §8), in one transaction, without touching `updated_at`.
  What the mapping cannot place is flagged `needs_review`: every coding
  preference (it may be one project's convention), and every project fact and
  decision (the project is not on record). On a copy of the field store, all
  225 memories were mapped, none to an illegal pair, and 185 of the 222 active
  ones were flagged.
- **Every memory the API returns carries it**: `kind`, `topic`, `applies_to`
  (`{"target": …, "project_name": …}`), `origin`, `goal_status` (goals only)
  and `needs_review`, in saves, reads, search hits, edits and the JSON export.
  Until the save path takes a structure (S1), a new memory's comes from its
  category, and re-filing it re-derives it.
- Domain: `Kind` (with the kinds each target allows), `Topic`, `AppliesTo`,
  `Origin`, `GoalStatus` and `MemoryStructure`, which refuses an illegal
  kind × target pair and a goal status on anything but a goal.

### Web UI: the memory list is paged

- The List tab renders 50 memories a page instead of every row at once, which
  made a store of thousands slow to open and to filter. Filter, category and
  sort still cover the whole store and return to page 1; a memory just saved or
  restored opens on its page. **Select all** now selects every memory the filter
  keeps, on every page, and says so ("Select all 60 matching").

## v0.5.0-rc5 — a web UI: a mental map of your memories, and the tools to curate them

Recall is unchanged from rc4. This release adds a browser UI at `/ui`, served by the daemon itself:
sign in with an API key, browse and search, correct, add and forget memories, manage your own
categories, edit many memories at once, and see the store as a map that zooms from categories to
clusters to single memories. See [`docs/web-ui.md`](web-ui.md).

- *On upgrade:* migration V10 adds the per-user category tables and seeds each user's list from
  `[understanding.taxonomy].extra_categories` plus the categories already on their memories (P3
  below). It is forward-only, like every migration: back up the store before upgrading if you might
  roll back to rc4.
- *On upgrade:* `/ui` is served by default (`[ui].enabled = true`) on the API port. It is static
  files only and uses the same `/v1` endpoints as any client, under the key's scopes, so it exposes
  nothing a key could not already do; set `enabled = false` to leave it unmounted.

### Web UI, phase P4: large stores, a layout that stays put, keyboard, colour vision

- **Large stores.** `GET /v1/map` takes `?category=` for one category's memories
  and their links. Above `[ui].map_detail_limit` (new, default 5 000), a
  whole-store map comes back as `"detail": "categories"`: counts only, with
  category links computed from each category's mean vector (`O(n·d)`, not `n²`).
  New fields: `detail`, `total`, `category`. The cache keeps whole-store and
  per-category answers side by side, and drops older generations.
  - In the UI, such a store opens on its categories. Opening one shows that
    category's own map, starting at its clusters. **All categories** goes back.
- **The layout runs in a Web Worker** (inline where workers are missing, as in
  jsdom), and **the browser keeps the last layout** of each map in
  `localStorage`: ids and coordinates only, cleared on sign-out.
  - Reopening an unchanged map takes 5–16 ms instead of 0.4–1.3 s.
  - After an edit, new or re-filed memories are placed by their pinned
    neighbours and nothing else moves. For 5 000 memories that takes 10 ms,
    where a fresh layout took 1.3 s.
- **Keyboard.** The map is focusable: `+`/`-` zoom, `0` fits, `1`–`3` jump to
  a level, the arrows pan, Escape clears a selection. Jumping to a closer level
  now closes in on the nearest category or cluster. Before, zooming in on the
  centre of the category ring could show an empty view.
- **Colour vision.** The palette was re-chosen so the built-ins stay at least
  ΔE 14 apart under simulated protanopia, deuteranopia and tritanopia (before:
  as close as 2.1). Every colour now keeps 3:1 on both backgrounds (one custom
  colour was 2.94:1 on white). A test enforces both.
- **Dark mode**: the map's labels follow a theme switch while it is open.
- **Lint**: `pnpm --dir web lint` (oxlint, with the React-hooks and jsx-a11y
  rules) gates CI and `just web-test`. It caught the map rebuilding its
  highlight set, and so re-styling every node, on every render.
- **Docs**: `docs/web-ui.md` gains screenshots from a demo store
  (`web/scripts/seed-demo.mjs`, `web/scripts/screenshots.mjs`), the keyboard
  table, large stores, dark mode and phone width.

### Web UI, phase P3: your own categories, and editing many memories at once

- **Each user's taxonomy lives in the database** (migration V10: `categories`,
  `category_seeds`), behind a `CategoryCatalog` port.
  - Every place that validates a category reads the caller's list: HTTP saves,
    edits and search filters, MCP (through HTTP), the verbatim ingestor, the AI
    classifier's prompt and schema, and consolidation's merge prompt. The
    pipelines take a per-call `TaxonomySource` instead of one list fixed at
    startup.
  - `[understanding.taxonomy].extra_categories` becomes a **seed**. The first
    time a user's list is read, it gets the extras plus every custom category
    already on their memories, once. On a copy of the field store, all 179
    active memories kept their categories.
- **Category endpoints:** `POST /v1/categories`, `PATCH /v1/categories/{id}`
  (relabel any; rename a custom one's id), `POST /v1/categories/{id}/merge`,
  and `DELETE /v1/categories/{id}` (`409` while non-empty, unless `?move_to=`).
  - A label is display-only and never re-embeds.
  - A rename or merge re-files the members through the same path as a bulk
    edit, and re-embeds them. On the field-store copy, renaming a 5-memory
    category took 68 ms in a debug build, and recall filtered to the new id
    found them at 0.81.
  - `GET /v1/categories` adds `default_label` and `known`. `/v1/map` shows the
    user's labels.
- **`POST /v1/memories:bulk`**: set the category or sub-label and add or remove
  tags on up to 500 memories.
  - All or nothing, in one transaction (`MemoryRepository::update_many`).
  - It embeds before it writes, so a failing embedder changes nothing.
  - One audit entry per memory with a shared detail.
- **The audit trail names the client.** REST writes record the
  `X-Recuerdos-Client` name when it is well-formed (the UI sends `web-ui`),
  else `rest`, as before.
- **UI:**
  - a **Categories** tab: create, relabel, rename, merge and delete, with
    counts, and each move spelled out before it happens;
  - **Select** mode in the list;
  - **Lasso select** on the map: Shift-drag, or the toggle. A lasso around a
    disc takes its members, and the selection lights up in place without
    burying the map in labels;
  - one bulk editor serves both.
- **Tests:**
  - Rust: 988 unit tests (catalog seeding and isolation, the curator,
    all-or-nothing bulk edits, taxonomy per user) and a new `tests/categories.rs`
    with 10 HTTP tests;
  - web: 162 tests, plus a fifth Playwright test (create → bulk-move → rename
    → recall → merge against a real daemon);
  - the eval baseline is unchanged (recall@5 83.3 %, P@1 73.1 %, MRR 77.7 %).

### Web UI, phase P2: the mental map

- **`GET /v1/map`**: memories, links between them, and the category summary in
  one answer.
  - Similarity links are **mutual k-nearest neighbours** with a floor above the
    model's similarity baseline, so a generic "hub" memory cannot wire itself
    to everything. On a 179-memory store: 278 links, at most 5 per memory, and
    6 memories with none.
  - Relation links come from the entity graph when it is enabled.
  - The links are cached per user until the audit trail moves.
    `VectorIndex::all`, `EntityGraph::memory_links` and
    `MemoryRepository::generation` are the new reads.
- **Map view at `/ui/map`** (Sigma.js 3 / WebGL + graphology, loaded only
  when the map is opened):
  - semantic zoom from categories through clusters to memories, each level
    sliding out of the one above;
  - deterministic ForceAtlas2 layout, with each category held together as an
    island;
  - clusters are sub-labels when most memories have one, otherwise Louvain
    communities named after their most shared tag;
  - every category has a colour, a glyph and a name;
  - category outlines at the cluster level;
  - hovering a memory shows its neighbourhood;
  - category, link-kind and minimum-similarity filters.
- **Recall preview**: run a question through recall from the map. It shows the
  results with relevance and which search found each one, lights them up in
  place at any zoom, and opens one on click.
- **Playwright end-to-end tests** (`web/e2e`): sign-in, edit, forget/undo, the
  map's real camera reaching each level, and the recall preview. They run in
  CI's Docker job against the release image.
- Bundle: 87 KB gzip up front. The map adds 22 KB and the renderer 34 KB, loaded
  on demand.

### Web UI, phase P1: correct, add and forget memories

- **Editor panel.** Open a memory to correct its text, category, sub-label and
  tags. Only the changed fields are sent.
  - Fields are validated on blur and on submit, against the daemon's own limits.
  - Typed text is never cleared on failure.
- **Conditional edits, end to end.**
  - `GET`, `PATCH` and restore answer an `ETag` (the memory's `updated_at`).
  - `PATCH` honours `If-Match` and answers **`412 precondition_failed`** when the
    memory changed since.
  - The editor then offers "Load the latest" or "Save mine anyway".
  - Without the header, `PATCH` behaves exactly as before.
- **`POST /v1/memories/{id}/restore`** undoes a forget. It keeps the id,
  `created_at` and history, and re-embeds and re-indexes the memory.
  - If re-embedding fails, the memory is forgotten again rather than left
    half-restored.
  - Audited as the new `restore` operation.
  - The UI's forget shows an 8-second **Undo** that calls it.
- **`GET /v1/categories`** lists the built-ins, the configured extras, and any
  stored category no longer configured, each with its label and the caller's
  count. Category wording now lives on `Category::label`, which the enriched
  vector header also uses; the embedded text is unchanged.
- **Add memory**, two ways:
  - saved exactly as written (`/v1/memories:direct`);
  - or through the AI pipeline (`POST /v1/memories`, `wait: true`), whose outcome
    is reported in words: saved N, already known, nothing lasting, or changed
    existing memories.
- A `read`-only key opens memories without edit controls.
- MCP: a `412` from the daemon maps to an invalid-request error, like `409`.

### Web UI, phase P0: sign in and browse at `/ui`

The first slice of [ai-specs/web-ui-plan.md](../ai-specs/web-ui-plan.md).

- **The daemon serves a browser UI at `/ui`.** It is a React + TypeScript app
  (`web/`), built with Vite and compiled into the binary via `rust-embed`
  behind a default `ui` cargo feature.
  - Sign in with an API key. The UI checks the key's shape on blur and verifies
    it with `GET /v1/ping`.
  - The header shows a badge with what the key may do (read-only, read & write,
    admin).
  - Memories appear as a list you can filter by text and by category and sort
    three ways.
  - On a daemon with `[auth].mode = "none"`, sign-in is skipped and a banner
    warns that anyone who can reach the port is the `default` user.
- **`[ui].enabled`** (default `true`). Off, `/ui` is not mounted.
- **Without the bundle, the daemon still builds.** If `web/dist` was never built,
  `/ui` answers `503` with the command that builds it (`just web-build`), so
  tests, the eval and the dev container need no Node.
  - Release binaries and the Docker image build the bundle first. The release
    workflow fails if it is missing.
  - CI's Docker smoke test checks that `/ui/` is the app.
- **Security.**
  - Strict CSP: `script-src 'self'`, `connect-src 'self'`, `frame-ancestors 'none'`.
  - `nosniff` and `no-referrer`.
  - The key is held in `sessionStorage` only.
  - Asset requests are not charged to the rate limiter.
- `GET /v1/memories/export?format=json` now includes each memory's `subcategory`.
- New `just web-build`, `just web-test` and `just web-dev` recipes.
- CI runs a `web` job: type-check, Vitest, build, and a 400 KB gzip bundle budget.
  The initial bundle is about 84 KB.

## v0.5.0-rc4 — graph off by default, enriched vectors by default, unused knobs gone

### Enriched vectors by default, and the rerank `replace` strategy removed

A second audit on the pipeline above (concept-recall-plan.md §11) found no unused settings left and two
changes worth making.

- **`[embeddings].document_text` defaults to `"enriched"`.** Re-measured on this pipeline it still wins every
  set: field precision@1 77.8 → **83.3**, preference 72.7 → **86.4**, broad-question on-topic@5 36.0 → **56.0**,
  eval precision@1 67.3 → **73.1** — six single-answer questions better, none worse. Its similarity baseline
  (0.53) comes from the per-model map. *Behaviour change on upgrade:* **a store built before rc4 is pinned to
  `content` and the daemon refuses to start** with the pin message. Run `recuerdos-ai reindex` once (daemon
  stopped), or set `document_text = "content"` to keep the old vectors. One trade to know about: relevance
  *numbers* read lower under enriched vectors, so a terse, keyword-free query against a near-empty store can
  fall under the 0.25 floor where it used to clear it ("package manager" against "User prefers pnpm":
  0.31 → 0.23) — while its *ranking* is better (the wrong memory topped "javascript package manager" under
  `content`). On the field store no single-answer expected memory falls under the floor.
- **`[rerank].strategy` is removed, and with it `replace`.** Raw rerank scores ordering the head lost on the
  eval in Phase 7 and on the field store now (seven answers moved, four up, three down); the stage is the
  weighted RRF leg, tuned by `[rerank].weight`. A config that still sets `strategy` gets the unknown-key
  warning. `field-measure.sh` M5 now reads rerank off-vs-on and weight 2-vs-1 instead.
- The reranker itself stays (off by default). On this pipeline it gains one field-store question and +1.9 eval
  precision@1 for ~395 ms p50 against ~8 ms.
- The eval baseline is re-recorded for the enriched default: recall@5 83.3, precision@1 73.1, MRR 77.7,
  complete@5 80.8.

### Graph off by default, the quality band removed, unused knobs gone

Measured on a frozen copy of a real 222-memory store — 18 field questions, 22 preference questions, 15 broad
questions — plus the eval set, each change switched off alone against the shipped pipeline
([`concept-recall-plan.md`](../ai-specs/concept-recall-plan.md) §10). Together, on that store: precision@1
**66.7 → 83.3** (field) and **63.6 → 86.4** (preferences), broad-question on-topic@5 **50.7 → 56.0**; 8 of 40
single-answer questions better, none worse; recall p50 unchanged.

- **The entity graph is now off by default (`[graph].enabled = false`).** On the real store it displaced
  better matches: off, five questions rank higher and none lower, including every question that names a
  project — the shape it was meant to help. It still wins the eval's multi-hop *relational* cases (recall@5
  85.7% on, 42.9% off), so the code stays: turn it on if your memories are people, teams and ownership
  chains. Off also means ingest asks the model for no relations. *Behaviour change on upgrade*: a config that
  does not set `enabled` loses the graph leg; `enabled = true` restores it exactly.
- **Confidence and importance no longer reorder recall.** Their shared multiplier was a feedback loop —
  importance rises with use, so a memory recalled often outranked better matches for every later question.
  Removing it: preference precision@1 72.7 → 86.4 (graph off), three questions better, none worse; the eval
  did not move. Both values stay on memories; the profile digest still orders by them.
- **The eval baseline is re-recorded** for the new defaults: recall@5 87.8 → 82.1 and complete@5 84.6 → 78.8,
  all of it the three relational cases the graph answers; precision@1 unchanged at 67.3.
- **Removed, measured or found unused:**
  - `[retrieval].hybrid` — never read; BM25 always ran. A config that still sets it now gets the
    unknown-key warning.
  - `[embeddings].query_instruction` (added in rc2) — no gain for bge-small on top of `enriched`.
  - `all-minilm-l6-v2` — the local model nobody calibrated; `bge-small-en-v1.5` is the one accepted.
  - `eval --grid`, `rerank distribution`, `scripts/rerank-grid.sh`, `just rerank-grid` — the Phase 7 tuning
    instruments; the tuning is done and `eval/grid-phase7.txt` keeps the last table. `rerank audit` and
    `recall audit` stay.
  - `ai-specs/experiments/recall-accuracy-2026-10-17.patch` — no longer applied to main.
- Measured and **kept**: recency decay (off helps one more preference question but costs the eval recall@5
  and `nl-category`), and the reranker (on the enriched field store, at ~600 ms p50, it reads p@1 77.8 vs
  61.1 off — the open trade). Exempting durable categories from recency was tried and not shipped: one
  question better, one worse.
- **The version number tracks the tag again.** `Cargo.toml` had stayed at `0.5.0-rc1` through the rc2
  and rc3 tags, so those binaries report `recuerdos-ai 0.5.0-rc1`; it is now `0.5.0-rc4`, and the release
  workflow refuses a tag whose version differs from `Cargo.toml`.
- **`scripts/field-measure.sh` clears its previous results before measuring**, so a step that is
  skipped (M3 with rerank off) no longer leaves an old run's file in the new bundle; its version
  defaults follow the release.
- CHANGELOG: `v0.4.0` moved back into version order.

## v0.5.0-rc2 / rc3 — the field host answered the questions rc1 asked, and recall reads a memory's labels

`v0.5.0-rc3` is `rc2` plus the release fix at the end of this list: **use rc3** — rc2's x86_64 Linux
binary needs glibc 2.43. Both binaries report `0.5.0-rc1` (fixed in rc4).


Deployed measurements are in. [`recall-quality-plan.md`](../ai-specs/recall-quality-plan.md) §17 now
carries the first readings taken against a live personal store running `v0.5.0-rc1`: 18 questions
written by the person whose memories they are, replayed over that store by
[`scripts/field-measure.sh`](../scripts/field-measure.sh) with the rerank stage off, at four values of
one knob. Three of rc1's owed questions have answers, and one of them is a shipped default.

- **`[retrieval].relevance_weight` now defaults to `0.5`, not `0.05`.** rc1 asked whether 0.05 was
  right or too timid; it was timid. Against the baseline it replaces, same build: **precision@1
  53.8 → 67.3**, **MRR 68.4 → 75.9**, recall@5 87.2 → 87.8, `complete@5` and abstention unchanged, and
  **no case kind's precision@1 fell** — four rose (`near-miss` 57.1 → 100.0, `statement` 25.0 → 75.0,
  `needle` 33.3 → 66.7, `nl-category` 50.0 → 62.5). Against `0.0` on that corpus the ladder reads
  +17.3 precision@1 and climbs monotonically through 0.05 / 0.2 / 0.3 / 0.5. On the real store:
  precision@1 **27.8 → 50.0** and MRR **47.7 → 63.4** across δ = 0 / 0.05 / 0.2 / 0.5. On neither
  instrument, at no value, did anything holding first place lose it — which is the sentence that
  mattered, because "the eval rewards relevance by construction" was the only reason 0.05 shipped.
  *Behaviour change on upgrade*: the order you get back changes. `relevance_weight = 0.05` restores
  what rc1 shipped, `0.0` restores the pre-Phase-2 order, and the accepted range is unchanged.
- **The reserved graph seat costs a little here, and the mechanism is visible in the audit.** With
  the seat on, precision@1 held (38.9 → 38.9) while MRR fell 57.9 → 55.7, twice, because an arrival
  the graph alone found enters at rank 2 by design and the second stage never judges it. Not a
  regression against rc1 — a measured price on a store whose entity graph is denser than the eval's.
  The default stays where rc1 put it, with the number recorded next to it.
- **`[rerank].mode = "off"` was the right default for a reason we had not published.** Fused as
  shipped (`strategy = "rrf"`, `weight = 1`) the stage changed almost nothing while costing 236 ms p50
  and ~1.4 GiB resident. Re-fused as `strategy = "replace"` on the same 18 questions it reads
  **83.3 precision@1 / 85.2 MRR** against the shipped fusion's 38.9 / 54.6 — eight queries gained
  first place, none lost it. That is a finding, not a shipped default: the eval grid has `replace`
  *losing* up to −9.6 precision@1 on three of four models, and the same field run shows how both can
  be true (where the model has nothing to say, its head scores 0.000 and banded raw scores reorder
  noise). What separates those two readings is a frozen copy of a real store inside the harness, and
  that is now the project's next measurement rather than a new phase.

- **New, opt-in: `[embeddings].document_text = "enriched"`** builds each memory's vector from a short
  header naming its category, sub-label and tags, then its sentence — so the vector leg, the one that
  matches on meaning, sees the labels the keyword leg already had. On a frozen 222-memory personal
  store, same model and ranking: 15 broad questions (*"how does the user approach testing?"*) put
  **50.7%** on-topic memories in the top 5 against 36.0%, and reach **57.2%** of their expected
  memories in the top 10 against 44.7%; 22 single-answer preference questions read precision@1
  **54.5 → 63.6** and MRR **72.5 → 79.2**; the 18 field questions **50.0 → 66.7** / **63.4 → 74.5**;
  the eval set **67.3 → 73.1** / **75.9 → 80.6** with abstention unchanged. No single-answer question
  ranked worse, and recall latency is unchanged — the header is embedded once, at save. The format is
  part of the collection pin (`bge-small-en-v1.5+enriched-v1`), so switching is a `recuerdos-ai
  reindex` with the daemon stopped, and it carries its own similarity baseline (`0.53`): **a config
  that sets `[retrieval].similarity_baseline` explicitly must change it too.** Default stays
  `content`, so an upgrade opens every existing store untouched.
  [`concept-recall-plan.md`](../ai-specs/concept-recall-plan.md) has the measurement and what comes
  next.
- **New, opt-in: `[embeddings].query_instruction`** prepends a retrieval instruction to recall
  queries, for models trained with one (BGE's *"Represent this sentence for searching relevant
  passages: "*, E5's *"query: "*), whichever provider serves them. Query-side only — no reindex. Off
  by default: for bge-small it adds nothing on top of `enriched`.
- **`recall audit --compare-config` refuses a second config with a different `document_text` or
  `query_instruction`.** One embedder serves both columns, so the second side's instruction would have
  been silently ignored; audit each config on its own instead.

- **Fixed: the `v0.5.0-rc2` x86_64 Linux binary did not start on the systems the README supports.** It
  failed with ``GLIBC_2.43' not found``: GitHub moved `ubuntu-latest` to 26.04 between rc1 and rc2, and
  a binary links against its build host's glibc. The release runner is now pinned to `ubuntu-24.04`, and
  a new release step refuses any Linux binary that needs a glibc above the documented 2.38 floor (rc1
  needed 2.38; rc2 needed 2.43). The rc2 Docker image and the macOS / aarch64 binaries were unaffected.

Nothing else moves: no migrations, no wire changes, no new downloads. Upgrading from `v0.5.0-rc1` to
a build containing this is a config-file question — the store is untouched, and every knob above stays
reversible (the document format by one `reindex`).

## v0.5.0-rc1 — Three ordering changes, and the measurements they still owe

A **prerelease**, deliberately: `latest` is not moved, the Python SDK is not
published, and what this tag is for is measurement rather than adoption.

[recall-quality-plan.md](../ai-specs/recall-quality-plan.md) Phases 0–3 are merged.
Each one shipped with a §4 gate whose second half is *"and the same comparison on a
real store"* — and every one of those halves has been blocked on the same thing: the
deployed server runs `v0.4.3`, which predates all three, so it has neither
`recall audit --compare-config` nor the two knobs the audits are meant to compare.
This build puts them on the same box as the store so the owed numbers can be read
off it. Until they are, treat what follows as *measured on the eval corpus, not yet
on your memories*.

- **`[retrieval].relevance_weight` = 0.05** (Phase 2, `v0.4.3..main`). After the
  index legs are fused, how well a memory matches the question now competes, a
  bounded amount, with how new it is: `score ×= 1 − δ·(1 − relevance)`. Before this,
  a memory matching the question *better* lost every close call to one three days
  newer. Eval: **precision@1 50.0 → 53.8**, MRR 63.9 → 66.6, recall@5 unchanged,
  abstention unchanged. `0.0` restores the old ordering exactly.
  [configuration.md](configuration.md#retrieval) explains why the shipped value is
  the smallest that clears the gate when the curve keeps climbing.
- **`[graph].expansion_slots` = 1** (Phase 3). A memory reached only by walking an
  entity relation brings one fusion term where a direct match brings two, so it
  cannot win a rank at any weight — now it reserves a *seat* instead: the
  best-connected memory you would otherwise not see is shown second, and your top
  result is untouched. Eval: **relational `complete@5` 57.1 → 85.7**, gated recall@5
  83.3 → 87.2, precision@1 unmoved. `0` gives back the `v0.4` pipeline, and nothing
  happens at all when a query names no entity the store knows — which, because seeds
  are exact, is most queries. One consequence to know about before it surprises you:
  a promoted row keeps its own `score`, so **the returned list is no longer sorted
  by score**, and a client that re-sorts by `score` undoes the feature.
- **A missed rerank deadline costs one chunk, never the next request** (Phase 1).
  The stage's model session used to be held by work that had already run out of
  budget, so one slow query could make the next one skip; the queue and the
  inference are now separately visible in the debug log and in `rerank audit`.

**Upgrade is ordinary and reversible.** No migrations landed since `v0.4.3`, so the
store file is untouched; pull the tag, recreate the container, and the volume stays
as it is. Rolling back is pulling `v0.4.3` again. If you want `v0.4`'s ordering while
keeping the tooling, set `relevance_weight = 0.0` and `expansion_slots = 0` — both
changes are config-reversible without another deploy.

What this tag is owed, and what your numbers will decide: whether 0.05 is the right
weight or too timid (the eval rewards relevance partly by construction, so a real
store is the falsifier); whether one reserved seat is the right seat count on a store
with thousands of entities rather than the eval's eighteen; and whether a hop ever
places something a human would call unrelated. The commands are in
[plan §17](../ai-specs/recall-quality-plan.md#17-results-log), and the findings go back
into that log before Phase 4 starts.

> **Answered on 2026-10-18**, by the box this tag was cut for: 0.05 was timid and the default is now
> `0.5`; one seat is affordable but not free; and the rerank stage turned out to be muted by its
> fusion rather than wrong. See the Unreleased entry above and §17 for the numbers.

## v0.4.3 — A release to publish the Python SDK

Nothing server-side changed. This cut exists because `release.yml` publishes the
Python client when a tag goes out, and the SDK's `0.4.0` — merged after
`v0.4.2` — would otherwise have sat unreleased on `main`, unnoticed by anyone
installing from PyPI.

**The Python SDK goes to 0.4.0.** `search_report()` returns what a search
response says *above* the results: the hits, the server's own `took_ms`, the
relevance floor that produced the list, and what the optional rerank stage did —
`applied`, `shadow`, `not_needed`, or `skipped` with a reason, absent when no
stage is configured (including any server older than `v0.4.2`). `search()` is
now `search_report().hits`, unchanged for callers who only want memories. Two
gaps turned up while doing it: the client had no `min_relevance` parameter at
all, though `docs/api.md` tells callers to read the floor and retry at `0.0`
([sdk-python.md](sdk-python.md)); and `recuerdos_ai.__version__` reported
`0.2.0` for two releases whose packaging said `0.3.0`, now pinned against the
installed distribution so it cannot drift again.

The crate is bumped to `0.4.3` so the binaries built from this tag report the
version they were tagged with. On the server side `v0.4.2..v0.4.3` is that one
line and nothing else.

## v0.4.2 — The second stage now says whether it ran

The same deployment that surfaced the v0.4.1 template defect turned up a second
one, and this one was not a config mistake: with `[rerank].mode = "on"`, its
reranker had never once contributed to a recall, and nothing a client could see
said so.

The stage misses its `[rerank].timeout_ms` by design and serves the pre-rerank
order — a second opinion is never worth a failed request — and the only trace
was a `WARN` in the journal. From the client's side that is indistinguishable
from `mode = "off"`. On that host, which also serves a 9B model and a local
embedding server, judging one candidate costs ~34 ms rather than the ~7 ms the
idle benchmark box managed, so a `depth = 10` head needs ~350 ms against a
250 ms deadline and every query worth asking lost the stage. `rerank audit`
aggravated it: measured deadline-free, it reported the stage as running while
the live path threw its answer away.

- **A recall that has a second stage now reports what it did**, as a top-level
  `rerank` object on `POST /v1/memories/search`: `applied` with how many
  candidates it judged and in how long, `shadow` with whether it *would* have
  moved the top result, `not_needed` when fewer than two cleared the floor, or
  `skipped` with `reason = "deadline"` or `"model"`. The field is **absent**
  when `mode = "off"`, so a deployment without a reranker gets the response it
  has always got, byte for byte. SDK models already tolerate unknown fields.
- **`rerank audit` says how many queries the stage actually contributed to**,
  and opens with that when it is not all of them. A run where the stage did
  nothing used to be indistinguishable from a run where reranking agreed with
  fusion, and "no disagreements found" is the wrong conclusion to draw from it
  ([evaluation.md](evaluation.md#judging-a-second-stage-on-your-own-memories)).
- **An eval whose stage was skipped says so once**, on stderr, instead of
  reporting reranking's measured gain as 0.0 and its cost as nothing.
- **`timeout_ms` is documented as what it is**: a tail bound measured on an idle
  machine, sized in pairs of milliseconds per candidate, with the two machines
  above side by side and a recipe — read `rerank.took_ms` across your own
  queries and set the deadline near twice that
  ([configuration.md](configuration.md#sizing-timeout_ms-for-your-machine)).

**For operators who turned reranking on and saw no difference:** send one search
and read `.rerank`. If it says `skipped`/`deadline`, raise
`[rerank].timeout_ms` to roughly 2× the `took_ms` you measure, or lower
`[rerank].depth` — understanding that the +5.8 precision@1 was measured at
depth 10, so a shorter head trades away part of the gain rather than getting it
cheaper.

## v0.4.1 — Two relevance settings that were being dropped

A review of a real deployment's `recuerdos-ai.toml` found that the example file
v0.4.0 shipped wrote `min_relevance` and `similarity_baseline` *after* the
`[rerank]` header. TOML cannot reopen a section, so both defined unknown
`[rerank]` keys instead of retrieval ones, and the loader ignored them without
complaint. `recuerdos-ai init` hands out that same file, so a copied template had
a relevance floor and a baseline that did nothing — including for the operator
whose config was being reviewed when it turned up.

- **The example is repaired.** Both keys are back under `[retrieval]`, with a
  note that a key written after the next `[table]` line belongs to that section.
  Pinned by a test that loads the shipped file, so the template cannot drift back
  into this. **If you copied the v0.4.0 example or ran `init`, move the two keys
  up yourself** — while they sat below `[rerank]`, changing them had no effect,
  whatever the file said.
- **A key nothing reads is now named at startup**, with the section it probably
  meant: `[rerank].min_relevance is not a known setting and is ignored — did you
  mean [retrieval].min_relevance?` Unknown keys stay non-fatal — a stray line in
  a config a working server has loaded should not become downtime — but they are
  no longer invisible, which is the only reason the example could carry this
  through review ([configuration.md](configuration.md#keys-nothing-reads)).
- **A first rerank-enabled boot explains its own pause.** The model load names
  the model and the cache directory *before* it begins, so a log that stops there
  is a ~1.1 GB download in progress, not a hang. `docs/deployment.md` also covers
  the bare-binary case, where `DynamicUser=` or `ProtectHome=` can leave the
  cache directory unwritable and the download has nowhere to land
  ([deployment.md](deployment.md#optional-second-stage-reranking)).

## v0.4.0 — Recall quality, and an optional cross-encoder reranker

Two measured recall defects are fixed, and a second stage that had been
built and rejected once before is added — this time with the measurements
that justify it.

A cross-encoder reads the query and one candidate *together*, which no
retrieval leg ever does. It was tried during the graph work and reverted:
neutral on a small corpus, a second gigabyte on the hot path. Before
trusting it again we fixed what made the fused list wrong in the first
place, grew the eval set to 108 memories and 59 cases, and ran 81 grid
cells — four models, two strategies, three depths, four weights. It earned
**+5.8 precision@1**. It also costs 1.1 GB of weights and +71 ms at p95, so
it ships **off by default**, with the tools to judge it against your own
memories before you spend either.

- **The keyword leg stopped matching stopwords.** "what do I use for data
  storage" matched any memory containing *the*, *do* or *use* — BM25 was
  indexing question words as if they carried meaning, which inflated noise
  into the top ranks. The analyzer now drops the Lucene English list plus
  question words, auxiliaries and pronouns, and deliberately keeps `use`,
  `user` and `prefer`, which are words people search for in a technical
  store. Measured on the committed corpus: `precision@1` **62.5% → 65.4%**,
  and abstention on questions the store cannot answer **25% → 75%** — a
  memory service that always answers something is one you stop trusting.
  The keyword index is versioned, so the first start after the upgrade
  rebuilds it from the `memories` table: no model calls, seconds at personal
  scale, and `reindex --text` does the same by hand
  ([deployment.md](deployment.md#6-upgrade)).
- **Relevance is calibrated per embedding model.** Relevance subtracts the
  cosine at which a model says nothing before rescaling, and that floor is a
  property of the model — it differs by more than **0.15** between the two
  shipped embedders. With one constant (0.5, measured against the default
  `bge-small-en-v1.5`), switching `[embeddings].model` silently miscalibrated
  every relevance number: either noise sailed past the floor or real answers
  were subtracted away. `[retrieval].similarity_baseline` now defaults from a
  measured per-model map (`gemini-embedding-2` → 0.46); a model with no
  measured entry falls back to 0.5 **with a warning at startup**, and
  `recuerdos-ai calibrate` produces the number. *Behaviour change if you use a
  hosted embedder: the relevance numbers move.*
- **`[rerank]` — an optional second stage, off by default.** `mode = "off" |
  "shadow" | "on"`. `shadow` scores every real recall and logs what *would*
  have changed (ids and ranks, never content) while serving the old order —
  how the stage is judged on traffic you didn't have to invent; `on` serves
  the reranked order. The tuned default (`bge-reranker-base`, fused as an
  RRF leg at weight 1.0, depth 10) measures **+5.8 precision@1, +2.7 MRR,
  +0.3 recall@5** with no case kind regressing, for **+71 ms p95** added to a
  recall, 1.1 GB of weights and ~1.5 GiB RAM per inference. `off` is a hard
  no-op: nothing loads, nothing downloads, the release image grows by nothing,
  and recall is byte-identical to a build without the feature. The model is
  English; the two multilingual candidates are measured and costed in
  [configuration.md](configuration.md#if-a-model-is-too-big-for-your-box).
- **`rerank audit` judges it on your store, not on our corpus.** It replays
  your queries over your real memories twice — with and without the stage —
  and prints both orderings, how often the head moved and what it cost. It
  writes nothing, so the daemon can stay running, and it is the only
  supported way to run the stage over an existing corpus: there is
  deliberately no "rerank everything" command, because a rerank score is a
  property of a query against a candidate list, not of a stored memory
  ([evaluation.md](evaluation.md)).
- **A slow reranker can cost latency, never a request.** Past `timeout_ms`
  (default 250, ~3.5× the tuned model's measured p95) the stage is dropped
  and the fused order is served with a warning. `max_concurrency` (default 1)
  bounds how many copies of the model your box holds, because each inference
  is another resident copy of the weights.
- **`matched.rerank_rank` and `rerank_score`** appear in a recall result only
  when the stage is on, so you can see which memories the second opinion
  promoted. Nothing a client has to send changed; the Python SDK gained the
  two optional fields.
- **Fixed: Gemini embedding batches larger than the API's cap.**
  `batchEmbedContents` accepts at most 100 inputs, so a `reindex` or a
  consolidation pass over a store of any size failed with HTTP 400. The
  adapter now chunks the input and preserves order.
- **Getting the weights where they need to be.** `warm-models` downloads the
  rerank model too when the config enables one, and the image can bake it in:
  `docker build --build-arg RERANK_MODEL=bge-reranker-base`. The default image
  carries no rerank model, since the default does not use one.

## v0.2.1

- **`[embeddings].max_batch_size`.** A remote embedding provider that caps
  its batch size — Alibaba DashScope rejects a batch larger than 10 — failed
  a `reindex` or a consolidation pass, which embed many texts at once, with
  `"batch size is invalid"`. The remote embedder now splits a large input
  into sequential requests of at most `max_batch_size` (default `0` = no cap,
  the previous send-it-all behaviour), preserving order. Set it to your
  provider's limit; the local provider ignores it.

## v0.2.0 — Graph (Strategy B): relational recall

Recall gains a third leg. Beside the semantic (vector) and keyword (BM25)
legs, a hop over an entity/relation graph reaches the memory that answers a
question it shares no words with — "who owns the billing service's data?"
surfaces the memory naming `ledger-db` by walking `billing service →
ledger-db`. **On by default**: the eval measured relational recall@5 at
**85.7%**, up from **71.4%** without the graph, with no other case kind
regressing.

- **A bi-temporal entity/relation graph.** Ingest extracts relations
  (`subject —predicate→ object`) alongside the entities it already pulled,
  and projects them into SQLite edge tables. Edges carry two clocks: *valid*
  time (when the fact was true in the world) distinct from the memory's
  *transaction* time (when we learned it). A superseding memory closes the
  old edge's validity interval instead of deleting it, so history is
  walkable, not lost.
- **Graph-hop retrieval as a third RRF leg.** Recall scans the query for
  entity mentions, keeps the ones some memory actually declared as seeds,
  and walks up to `[graph].max_hops` edges. It is additive and
  self-silencing: a query naming no known entity yields the exact two-leg
  result, so non-relational recall cannot regress — a property the eval and
  an empty-leg identity test both hold.
- **Tuned so recall's gain isn't precision's loss.** At full weight the hop
  let a relation "bridge" outrank the direct answer on one case, costing
  `precision@1`. `[graph].rank_weight` (default `0.3`) down-weights the hop's
  fusion term so it *adds* relational recall (85.7%) without *displacing* a
  strong direct match — `precision@1` stays at its pre-graph 62.5%.
  `[graph].unanchored_floor` (default `3`) additionally caps a memory only
  the graph found, so it can't leapfrog a direct hit while still reaching
  the top-k.
- **Time-travel recall.** `as_of` (REST search and the `memory_recall` MCP
  tool) reads the hop in valid time — "who was on-call before the reorg?" —
  traversing edges that were live then. `graph_rank` in a result's `matched`
  object says when a hop, rather than a semantic or keyword match, is why a
  result appeared.
- **Budgeted, resumable backfill.** `recuerdos-ai graph backfill
  --entities` (zero model calls) and `--relations` (bounded by
  `[graph].backfill_budget`, resuming where it stopped) give edges to a
  corpus that predates the graph; `--dry-run` spends nothing. See
  [deployment.md](deployment.md).
- **Off is a clean switch.** `[graph].enabled = false` restores the
  pre-graph behaviour exactly — no hop, no edges written, no
  relation-extraction tokens — for a deployment that wants neither.

## v0.1.1

- **`[server].mcp.allowed_hosts`.** The streamable-HTTP `/mcp` endpoint's
  DNS-rebinding guard accepted only loopback `Host` headers, so reaching it
  over the network needed a reverse proxy that rewrites `Host`. This option
  lets an operator allow the hostname(s) clients connect with directly
  (added to the loopback defaults, so local access still works); a single
  `"*"` disables the guard for a trusted private network. Empty — the
  default — is unchanged.
- **Docs & install hardening** since v0.1.0: reverse-proxy setups must
  rewrite `Host` for `/mcp` (Caddy `header_up Host localhost`); the Linux
  binary's glibc-2.38 floor (imposed by ONNX Runtime's prebuilt) is
  documented and `install.sh` now checks it up front and points to Docker
  below it; the GHCR image name is lowercased; and the README gained
  Updating and Uninstalling sections plus a comparison and use cases.

## v0.1.0 — Phase 6: SDK, docs & release

The first release candidate. Mostly about making everything from Phases
0–5 installable, documented, and usable from something other than curl —
plus the provider choice and operational features a self-hosted
deployment needs.

- **Python SDK** (`pip install recuerdos-ai`): a thin typed client over
  the REST API — one method per endpoint, pydantic models, and no
  caching or retry opinions of its own. The two things it does add are
  the two a raw `httpx` call makes awkward: errors keyed off the API's
  stable error `code` so callers branch on a type, and job polling.
  Ships a LangChain `BaseRetriever` behind a `[langchain]` extra.
- **Integration recipes** for the client types without one: Hermes,
  LangChain/LangGraph, and a plain-REST recipe for anything else. Every
  request and response in them was captured from a running daemon.
- **Benchmarks** in `docs/performance.md`, measured against the release
  image rather than a dev build, with the methodology and the hardware
  written down beside the numbers — including the targets that were
  missed.
- **`install.sh`**, which downloads a binary, verifies its checksum, and
  refuses to install anything unverified. Tested against a tampered
  tarball and a missing checksum, not just the happy path.
- **Release workflow**: native builds per architecture (linux
  x86_64/aarch64, macOS arm64), multi-arch image to GHCR, drafted
  release notes.
- **Apache-2.0**, and a recorded name-availability check.
- **README** rewritten as a launch page.

And the deployment-facing features that landed alongside the release work:

- **External embedding providers.** Beyond the built-in local ONNX model,
  embeddings can come from a **native Gemini client** (task-type-aware —
  `RETRIEVAL_DOCUMENT` on store, `RETRIEVAL_QUERY` on search, for better
  recall), any **OpenAI-compatible** endpoint, or **Ollama**. Chosen with
  `[embeddings].provider`; the model's dimensionality is discovered by a
  real request at startup, so there is no width to configure.
- **`recuerdos-ai reindex`.** Change the embedding model or provider and
  re-embed every memory in place — in one transaction — instead of
  needing a fresh data directory. The store pins its model, and the daemon
  now **refuses to start on a mismatch** with a message naming the fix,
  rather than failing later on the first recall.
- **Streamable-HTTP MCP transport** at `/mcp` (previously deferred, see
  Phase 3). A client connects over HTTP with a bearer token — no local
  binary, no `docker exec` shim — sharing the same four tools and the same
  per-user auth as the stdio path. This is the natural way to reach a
  daemon running on another host.
- **Gemini as a reasoning provider** for `[understanding]` — a preset over
  the OpenAI-compatible client, since Gemini's chat API needs no native
  client the way its embeddings do.
- **`recuerdos-ai config`** prints the effective, secret-free configuration
  (which providers, models and transports are actually in force), and
  `RECUERDOS_AI_CONFIG` points the whole CLI at one file without repeating
  `--config`.

## v0.5.0-alpha — Phase 5: Consolidation

Memory that stays clean over time. Until now the store only grew; this
phase is everything that makes it shrink again.

- **`POST /v1/sessions/distill`** and the **`session_distill` MCP tool**
  reduce a finished session to the few things that outlive it. Same
  pipeline as ordinary ingestion, asked a stricter question — *what is
  still true after this session ends?* — so the conventions and root
  causes survive and the task chatter does not. Most sessions distil to
  nothing, which is the correct answer.
- **A nightly consolidation job**, also runnable as
  `recuerdos-ai consolidate [--dry-run]`. Near-duplicates within a user's
  category are grouped by similarity, and the model writes the one memory
  that says what all of them said. Five phrasings of one preference
  become one active memory and five superseded ones, each with a `merge`
  audit entry carrying the reasoning.
- **The threshold proposes, the model disposes.** Similarity gets
  memories into a cluster but cannot tell whether they mean the same
  thing — "prefers pnpm" and "prefers Vitest" are very close in embedding
  space and are two facts. The merge prompt argues *against* merging, and
  every unusable answer parses as keep-separate: a duplicate surviving
  one more night is cheap, a merged-away fact is not.
- **TTL expiry.** Memories past their `expires_at` are retired with an
  audit entry naming the date. Retired, not erased — `expires_at` is a
  promise a memory stops being used, not that it stops existing.
- **Importance decay.** Memories are rescored from how recently and how
  often they were actually recalled, feeding recall ranking. It only ever
  demotes, and never below a floor well above zero: an architecture
  decision nobody has read this year has to lose ties, not vanish.
- **An LLM-written `memory://profile`.** Generated per half (how they
  work / about them) and cached until the memories under it change, so it
  compresses forty preferences into a paragraph instead of truncating at
  eight. Staleness is derived from the memories themselves rather than a
  dirty flag every write path has to remember to set.
- **Graceful degradation throughout.** Expiry and decay need no provider
  and run everywhere. Merging and the written digest need one and are
  simply skipped without it — the profile falls back to assembly, and the
  resource contract does not change. Distillation is the single exception:
  it refuses rather than storing a transcript whole, because that memory
  would be unrecallable and would cost a context window on every match.

## v0.4.0-alpha — Phase 4: Understanding

The differentiator: raw text in, understood memories out. A memory store
stops being a log.

- **`POST /v1/memories`** accepts raw content and returns `202` with a job
  id; **`GET /v1/jobs/{id}`** reports what became of it. The work is a
  model call that takes seconds — holding the request open for it would
  make a client's timeout our problem and lose the work on a disconnect.
- **Extraction** splits content into atomic, separately-recallable
  memories with a category, tags and entities. One sentence about moving
  hosts *and* a testing preference becomes two memories, not one blob.
- **Reconciliation** decides ADD / UPDATE / DELETE / NOOP against the
  memories most similar to each candidate. A contradiction supersedes
  what it replaces instead of accumulating beside it, so recall stops
  returning last quarter's answer alongside this quarter's. Superseded
  memories are retained for audit and reachable with
  `include_superseded=true`.
- **Three providers** — Anthropic, any OpenAI-compatible endpoint, and
  Ollama — behind one `ChatModel` contract, with retry, timeout and
  malformed-JSON recovery shared across all of them so behaviour cannot
  drift between them.
- **A durable job queue and worker pool.** Jobs survive a restart, a
  crashed worker's job is reclaimed rather than lost, and a poison job
  dead-letters with its error instead of retrying forever.
- **Still zero-egress by default.** `[understanding].provider` defaults to
  `none`, and in that mode the same endpoints store content verbatim,
  inferring only the category from unambiguous phrasing. Turning a
  provider on requires no client changes.
- **MCP `memory_save` runs the pipeline**, so an agent's save is
  reconciled against what is already stored — and the tool says so
  honestly, including when nothing was stored because the store already
  knew it.
- **`recuerdos-ai eval`** scores retrieval quality against a committed
  baseline, and CI fails a PR that drops recall@5 by more than five
  points. Nothing else in the suite would notice a ranking regression.
  See [evaluation.md](evaluation.md).

Known limitation: one eval case is committed failing — a query with no
vocabulary in common with its memory ("book a restaurant" → "user is
vegetarian") does not reach the top 5. It is kept as a real target rather
than removed to flatter the score.

## v0.3.0-alpha — Phase 3: MCP server

Claude Code, opencode and any other MCP client can now read and write the
same memory store.

- **Three tools** — `memory_save`, `memory_recall`, `memory_forget` — and
  the **`memory://profile` resource**, served by `recuerdos-ai mcp`.
- **The stdio server is a shim**, forwarding to the daemon over the same
  authenticated REST API any client uses. Four editor windows would
  otherwise mean four resident embedding models and four writers on one
  SQLite file.
- **Tool descriptions carry the trigger logic.** A model decides from the
  description alone whether to save something, so they name concrete
  phrasings ("I prefer", "we decided") *and* say when not to call —
  without which a memory store fills with transient task chatter.
- **`memory_forget` is two-step and the server enforces it**: ids without
  `confirm: true` delete nothing, and the candidate listing states
  plainly that nothing has been deleted yet.
- **`GET /v1/profile`** exposes the same digest over REST, which is what
  the shim reads and what a Claude Code SessionStart hook can curl.
- Integration recipes for [Claude Code](integrations/claude-code.md)
  (including SessionStart and PreCompact hooks) and
  [opencode](integrations/opencode.md).

Deferred at the time: the streamable-HTTP MCP transport. rmcp's session
factory cannot see request headers, so per-user auth would need a per-call
path that complicates keeping tool definitions identical across transports
— and every target client then spoke stdio. **Shipped later in v0.1.0**
(see above): the HTTP handler forwards the request's bearer to the same
per-user REST auth, so both transports share one set of tool definitions.

308 tests.

## v0.2.0-alpha — Phase 2: Memories

Store memories and search them semantically and lexically, fully offline,
strictly per user.

- **Hybrid recall.** A vector leg (sqlite-vec) and a keyword leg
  (tantivy BM25) run on every search, fused by reciprocal rank. The
  keyword leg is what finds `useQuery` as a literal token; the vector leg
  is what answers a question sharing no words with the memory.
- **Local embeddings**, in-process ONNX (bge-small-en-v1.5, 384d), baked
  into the Docker image so a container never downloads at runtime and an
  air-gapped host works. `recuerdos-ai warm-models` pre-populates a cache.
- **Taxonomy**: eight built-in categories, extensible via config. Unknown
  names are rejected rather than silently created.
- **REST**: `POST /v1/memories:direct`, `POST /v1/memories/search`,
  `GET/PATCH/DELETE /v1/memories/{id}`, `GET /v1/memories/export`,
  `GET /v1/audit`.
- **Export** as markdown (grouped by category, one memory per line, so it
  stays greppable) or JSON — your memories are yours and portable.
- **Soft delete and supersede** both retain the row, so the audit trail
  stays truthful.
- Isolation extends to memories: `user_id` is a vec0 *partition key*
  (a query never scans another user's vectors), each user has their own
  tantivy index directory, and the cross-tenant suite now tests recall,
  by-id reads, edits, deletes, export and audit.

**A 12x latency fix, found by measuring rather than assuming.** The first
benchmark showed 251ms end-to-end searches against 8ms of actual work:
argon2id was running on every request and accounted for 96% of the
response. `VerifiedKeyCache` caches the verification (SHA-256 digest of
the presented secret, constant-time compared) while still reading the key
row every time, so revocation and scope changes take effect immediately.

Measured after, at 2,000 memories: **p95 29ms end-to-end**, 16ms
server-side, 55ms per ingest.

Known limitations, stated rather than buried:

- The plan's 100k-memory target is unproven. sqlite-vec 0.1.x has no ANN
  index — KNN is a brute-force scan — so recall latency grows linearly
  with corpus size. `scripts/bench-recall.sh` takes the scale as an
  argument.
- Search filters are applied after the indexes answer, so a very
  selective filter over a large corpus can return fewer results than
  asked for.
- No generated OpenAPI spec yet; `docs/api.md` is hand-written. The spec
  belongs with Phase 6's SDK, which is what consumes it.
- Ingest commits the text index synchronously (read-after-write beats
  throughput for an agent that just saved something), which is most of
  the 55ms.

The release image grows from ~112 MB to ~287 MB: the embedding model is
baked in (~130 MB) so a container never downloads at runtime, plus the
C++ runtime libraries ONNX needs. Both build stages now pin the same
Debian release — `rust:1-slim` had drifted to trixie while the runtime
stage was still bookworm, which produced a binary the runtime's glibc
could not load.

297 tests.

## v0.1.0-alpha.1 — Phase 1: Identity

Multi-user authentication. Every `/v1` route now resolves to exactly one
user, and the isolation guarantee is enforced by the type system.

- **Users and API keys.** `recuerdos-ai user add|list`,
  `key issue|list|revoke`. Keys are `ra_live_<prefix><secret>`; only an
  argon2id hash is stored, and the key is displayed once at issue time.
- **Scopes**: `read`, `write`, `admin`. `write` does not imply `read`, so
  a write-only ingestion key is expressible. `admin` implies everything.
- **`UserContext`**, the capability token every later context will require.
  Its constructors are `pub(in crate::identity)`, so no other context can
  invent one for a user it did not authenticate — cross-tenant access is a
  compile error. `check-boundaries.sh` fails if that visibility is widened.
- **Auth middleware**: `Authenticated`, `ReadAccess`, `WriteAccess`
  extractors. The argon2 verify runs on `spawn_blocking`; `last_used_at`
  is recorded off the request path.
- **Uniform rejections**: missing, malformed, unknown, wrong-secret and
  revoked keys are byte-identical 401s, and the verify runs before the
  revocation check so timing doesn't leak revocation either.
- **Error envelope** `{"error":{"code","message"}}` for all contexts;
  `internal` responses never carry their detail.
- **`[auth].mode = "none"`** for single-user deployments: a real persisted
  `default` user (so data isn't orphaned if auth is re-enabled), a loud
  startup warning, and fail-closed parsing of the mode value.
- **SQLite storage**: WAL, enforced foreign keys, refinery migrations run
  at startup.
- `tests/identity_isolation.rs`, the cross-tenant suite later phases
  extend. Its sharpest case splices one user's public prefix onto
  another's secret and asserts 401.
- Docs: `api.md`, `security.md` (isolation + threat model),
  `configuration.md` auth/storage sections, `architecture.md` worked
  example.

130 tests.

## v0.1.0-alpha.0 — Phase 0: Foundation

- Screaming-architecture skeleton (identity/memories/understanding/
  providers/consolidation, each domain/application/infrastructure) plus
  the shared kernel (`RaError`, UUIDv7 ids, `Clock`).
- `scripts/check-boundaries.sh` boundary check, wired into `just check`.
- Docker-only dev flow: `Dockerfile.dev` (non-root, host-UID-matched),
  `docker-compose.yml`, and a multi-stage release `Dockerfile` (~112 MB,
  non-root, healthchecked).
- `AppConfig`: TOML + `RECUERDOS_AI_*` env overrides via figment,
  aggregated validation errors, `recuerdos-ai init`.
- axum HTTP skeleton: `/healthz`, `/version`, graceful shutdown on
  SIGINT/SIGTERM, tracing (plain or JSON via `RECUERDOS_AI_LOG=json`).
- GitHub Actions CI: check / test / docker-build-and-smoke-test.
