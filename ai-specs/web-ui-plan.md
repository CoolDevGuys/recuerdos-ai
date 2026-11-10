# Recuerdos AI — Web UI: a mental map of memories, and the tools to curate it

Opened **2026-11-10**. Owner request: *"Introduce a web UI to let users visualize and manage their
memories. It should display a dynamic graph of the stored memories and their relationships, group
them by category, and use zoom as the grouping mechanism: zoomed out, the categories are shown; zooming
in, the categories 'dissolve' into the individual memories — a representation of the mental map. For
management, the user can edit or correct any memory or category, and add or delete memories."*

Status: **P0 and P1 implemented** (2026-11-10, branch `feat/web-ui`); P2–P4 planned. Phases are ordered so
each one ships something usable (§9). §9.1 records how P0 differed from this plan.

---

## 1. Decisions taken

These were settled with the owner on 2026-11-10 and are not reopened in this plan.

| # | Question | Decision | Consequence |
|---|---|---|---|
| D1 | What draws the lines between memories? | **Semantic similarity + entity graph.** | Similarity edges from the stored vectors are always available. Entity-graph relations are drawn as a second, visually distinct edge type when `[graph]` is enabled and populated. |
| D2 | Where does the UI live? | **Embedded in the daemon.** | A static SPA is compiled into the binary and served at `/ui` on the API port. One binary, one deploy, same origin, no CORS. |
| D3 | How does a user sign in? | **Paste an API key.** | No new identity code. `GET /v1/ping` already returns `user` + `scopes`; `read` sees the map, `write` edits, `admin` everything. |
| D4 | What does "edit a category" mean? | **All four:** re-categorise one memory; bulk move / merge; create / rename custom categories; rename built-ins (display label). | The taxonomy moves from the config file into the database, per user (§4.2). This is the largest backend change in the plan. |
| D5 | Frontend stack | **React 19 + Vite + TypeScript + Tailwind v4, pnpm.** | Next.js is not used: an embedded static build cannot use its server features. |
| D6 | Map rendering | **Sigma.js (WebGL) + graphology**, ForceAtlas2 layout. | Smooth zoom to thousands of nodes. The "dissolve" is done with node/edge reducers keyed on the camera ratio (§5.2). |
| D7 | Adding a memory | **Both, the user chooses.** | Default is a direct save: exact text with the user's category and tags, instant, no LLM. A "let the AI classify it" toggle routes it through the understanding pipeline instead (async job, polled). |
| D8 | Deleting a memory | **Existing forget + confirm + undo toast.** | `forget` is already a soft delete (`memory_forgetter.rs`). Undo is a small `:restore` endpoint that keeps the id, timestamps and edges, rather than re-saving a copy (§4.4). |

## 2. Goals and non-goals

**Goals**

- **G1 — See the shape of what is remembered.** Categories, their sizes, how they relate, and the
  clusters inside them, at a glance.
- **G2 — Zoom is the grouping control.** Zoomed out: one node per category. Zooming in: sub-labels.
  Further in: individual memories and their edges. Each transition animates.
- **G3 — Curate in place.** Edit, re-categorise, tag, add, forget and restore memories from the map or
  from a list view, with the change visible on the map immediately.
- **G4 — Curate the taxonomy.** Create, rename, merge and delete custom categories, and relabel
  built-ins, per user.
- **G5 — Debug recall visually.** Type a question, see which memories recall returns, where they sit on
  the map, and why (`matched.vector_rank`, `text_rank`, `graph_rank`).

**Non-goals (for this plan)**

- Multi-user collaboration or sharing between users. Isolation between users stays absolute.
- Editing the entity graph by hand (adding or removing relations). It is shown, not curated; a later
  plan can add it.
- A mobile-first layout. The map targets desktop. The list view and the editor must still work at
  phone width.
- Replacing the MCP tools or the CLI. The UI is a third client of the same HTTP API.

## 3. Architecture

```
browser ──/ui/*──────────────▶ daemon: static assets (rust-embed, from web/dist)
        ──/v1/* (Bearer key)─▶ daemon: existing API + the new endpoints in §4
```

- **Source:** `web/`, a pnpm project at the repo root: `web/src`, `web/package.json`,
  `web/vite.config.ts`. Built to `web/dist`.
- **Embedding:** `rust-embed` (or `include_dir`) under a cargo feature `ui`, on by default.
  - A `--no-default-features` build skips the asset step, so backend-only contributors and the
    eval/bench paths do not need Node.
  - If `web/dist` is missing while `ui` is on, the build still succeeds (`allow_missing`), and `/ui`
    answers `503` naming `just web-build`. Release builds check that the bundle is present (§9.1).
- **Routing:**
  - `GET /ui` and `GET /ui/*` serve the asset. An unknown path under `/ui/` serves `index.html`, so
    client-side routes work on reload.
  - Hashed assets get `Cache-Control: immutable`; `index.html` gets `no-cache`.
  - Static routes carry no auth. They contain no data.
- **Config:** `[ui] enabled = true` (default). The mount is fixed at `/ui`, because the bundle's
  asset URLs are built against it. Disabled, the routes are not mounted, and `/ui` answers like any
  unknown path (the API asks for a credential before routing, so that is a 401).
- **Security headers on `/ui`:**
  - Strict CSP: `default-src 'self'; connect-src 'self'; img-src 'self' data:; style-src 'self'
    'unsafe-inline'` (Tailwind) and `frame-ancestors 'none'`.
  - No third-party CDN. Fonts are self-hosted or system fonts.
- **Auth mode `none`:** the login screen is skipped and a banner repeats the server's warning that
  anyone who can reach the port is the `default` user.
- **Frontend layering** follows the owner's standing rule that UI components are thin: no business
  logic and no direct network requests.
  - `web/src/api/` is the only module that calls `fetch`. It is a typed client generated from or
    checked against the DTOs.
  - `web/src/domain/` holds pure functions: map building, zoom-level maths, cluster summaries,
    validation.
  - `web/src/state/` is the store and query cache (TanStack Query for server state; a small store for
    UI state).
  - `web/src/components/` contains only presentation.

## 4. Backend additions

All new endpoints:
- are under `/v1` and per-user (every query starts from `UserContext`);
- write audit entries with actor `ui` (the client name is sent as `X-Recuerdos-Client: web-ui`);
- validate and parse at the boundary;
- appear in `docs/api.md`.

### 4.1 The map: `GET /v1/map`

One request returns everything the map needs, so the client does not issue N similarity searches.

```jsonc
{
  "generation": "c0ffee…",            // changes on any write; used for caching and stale detection
  "nodes": [{ "id", "content", "category", "subcategory", "tags", "confidence",
              "created_at", "updated_at", "superseded_by" }],
  "edges": [{ "source", "target", "kind": "similar", "weight": 0.71 },
            { "source", "target", "kind": "relation", "predicate": "uses", "entity": "pnpm" }],
  "categories": [{ "id", "label", "built_in", "count",
                   "subcategories": [{ "name", "count" }] }],
  "category_edges": [{ "source", "target", "weight" }]   // aggregated cross-category similarity
}
```

**Similarity edges, and the hub problem**
- Build a **mutual k-nearest-neighbour** graph: k = 5 by default, query param `k` from 1 to 15. Keep
  an edge only if each memory is in the other's top-k and the cosine is above a floor.
- The floor reuses the model's `similarity_baseline` from the pin map (0.53 for
  `bge-small+enriched-v1`), plus a margin. That keeps the threshold calibrated per model, the way
  recall already is.
- The *mutual* part matters. Recall shows hub memories: the short pnpm preference ranks near the top of
  most "the user prefers…" questions. Plain kNN would wire a hub to everything and pull the layout into
  a star. Mutual kNN drops those one-sided links.

**Cost**
- About 200 memories × k queries against sqlite-vec, or an in-memory dot product over the loaded
  vectors: well under 100 ms.
- The result is cached per user and keyed by `generation`.
- Target budget is **< 300 ms for 1 000 memories**. Above 5 000, the endpoint returns category and
  sub-label aggregates only. The client asks for one category's nodes with `?category=` when it zooms
  in (§5.2, level 2).

**Relation edges**
- Come from `EntityGraph` when `[graph].enabled` and the user has relations. Otherwise they are
  omitted, and the client hides the "relations" toggle.
- `as_of` is accepted and passed through, which allows a "what did my map look like then" view later.

**Category edges**
- The sum of similarity-edge weights between members of two categories, normalised by size.
- These are the only edges drawn at the outermost zoom.

**Superseded memories** are excluded unless `include_superseded=true`. The UI shows them faded and
linked to their replacement.

**`generation`** is a per-user counter bumped by every write path: save, update, forget, restore,
bulk, taxonomy change, consolidation and reindex. It is stored in a small table rather than computed
from timestamps, so that deletions count too.

### 4.2 The taxonomy moves into the database

Today:
- the 8 built-in categories are an enum;
- custom categories come from `[taxonomy].extra_categories` in the config file, shared by every user
  on the daemon;
- `Category::parse_with_extras` validates against that list.

D4 needs per-user, editable categories, so:

- **Migration `V10__categories.sql`.**

  ```sql
  CREATE TABLE categories (
      user_id    TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
      id         TEXT NOT NULL,          -- 'preference.coding', 'health.sleep'
      label      TEXT,                   -- display override; NULL = default label
      built_in   INTEGER NOT NULL,
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL,
      PRIMARY KEY (user_id, id)
  );
  ```

  - Built-ins get a row only when relabelled. Their ids never change.
  - The migration seeds every user's custom categories from `extra_categories`, plus any custom
    category id already present in that user's `memories`. No memory becomes invalid.
- **Domain:**
  - A `Taxonomy` port (`taxonomy_repository.rs`) replaces the `extra_categories` slice threaded
    through handlers.
  - `Category::parse_with_extras` becomes `Taxonomy::resolve(context, raw)`.
  - `[taxonomy].extra_categories` stays accepted as a **seed for new users**. That keeps existing
    configs working and lets an operator pre-provision categories. It is documented as such, not
    removed.
- **What a rename does to vectors:**
  - Relabelling a **built-in** is display-only. The enriched header (`document_text.rs::describe`)
    keeps using the stable built-in wording, so vectors stay comparable and the pin stays valid.
  - **Renaming a custom category's id** rewrites `memories.category` for its members, and re-embeds
    them, because the enriched header embeds the custom name. Re-embedding runs in the same request
    for up to a few hundred memories. Above that, it is a job the UI polls via `/v1/jobs/{id}`,
    reusing the existing job machinery.
  - Changing only a custom category's display **label** is display-only.
- **Endpoints:**

  | Method & path | Purpose | Scope |
  |---|---|---|
  | `GET /v1/categories` | All categories for the user, with counts and sub-labels | read |
  | `POST /v1/categories` | Create a custom category `{id, label?}` (id validated like today) | write |
  | `PATCH /v1/categories/{id}` | `{label}` for any category; `{id}` (rename) for custom only | write |
  | `POST /v1/categories/{id}:merge` | `{into}`: move every member to `into`, then delete `id` (custom only) | write |
  | `DELETE /v1/categories/{id}` | Custom only. Refused with 409 if non-empty, unless `?move_to=` is given | write |

- **MCP and the understanding pipeline** read the same `Taxonomy`. That way a category created in the
  UI is immediately valid for `memory_save`, and is offered to the classifier prompt. That prompt
  currently lists the config extras; check `understanding/prompts` and keep its length budget.

### 4.3 Bulk edits: `POST /v1/memories:bulk`

```jsonc
{ "ids": ["…"], "set": { "category": "skill", "subcategory": null },
  "add_tags": ["rust"], "remove_tags": ["misc"] }
```

- **One transaction:** all ids belong to the caller, or the request is refused (404 with the
  offending ids). There are no partial writes.
- **Re-embeds** only what changed embedded text. `DocumentText::embeds_labels()` already says whether
  label edits do.
- **Limits:** 500 ids per request.
- Writes **one audit entry per memory** with a shared `batch` correlation id.

### 4.4 Restore: `POST /v1/memories/{id}:restore`

- Clears the soft-delete marker.
- Re-inserts the memory into the vector and text indexes. The vector is re-embedded, since forget
  drops it from the index.
- Re-records entity edges.
- 404 if the memory is not the caller's or was never forgotten; 410 if it has been purged.

The UI's undo toast (8 s) calls this. It also enables a later "recently forgotten" view at no extra
cost. Whether forgotten rows are ever purged is not changed by this plan.

### 4.5 Optimistic concurrency

The UI is a long-lived editor, and the MCP tools and consolidation can change a memory under it. So:
- `PATCH /v1/memories/{id}` and the category endpoints accept `If-Match: "<updated_at>"` and return
  412 on a mismatch.
- The UI then shows "changed elsewhere — reload / overwrite".
- The header is optional, so existing clients are unaffected.

### 4.6 Already there, reused as-is

| Need | Endpoint |
|---|---|
| Who am I, what can I do | `GET /v1/ping` (`user`, `scopes`) |
| Read one memory | `GET /v1/memories/{id}` |
| Edit content / category / sub-label / tags / expiry | `PATCH /v1/memories/{id}` |
| Forget | `DELETE /v1/memories/{id}` |
| Direct add | `POST /v1/memories:direct` |
| AI-classified add | `POST /v1/memories` → poll `GET /v1/jobs/{id}` |
| Recall preview | `POST /v1/memories/search` (its `matched` ranks drive the "why" panel) |
| History of a memory | `GET /v1/audit` |
| Download everything | `GET /v1/memories/export` |

## 5. The mental map

### 5.1 Layout

- A single graphology graph holds **every memory node**, each with an extra attribute `cluster` set
  to `category`.
- **ForceAtlas2** runs in a Web Worker (`graphology-layout-forceatlas2/worker`), with:
  - `linLogMode` for tighter clusters;
  - an extra **cluster gravity** term pulling each node towards its category's centre;
  - category centres placed first by a small force layout over `category_edges`. Related categories
    therefore sit near each other, and the zoomed-out view is itself meaningful.
- **Stability:** positions are seeded deterministically from the memory id hash, and the last layout is
  cached in `localStorage` keyed by `generation` (wrapped in try/catch). Reopening the map does not
  reshuffle it. After an edit, only the touched nodes are released, and the layout runs a few hundred
  iterations warm.
- Node **size** follows `confidence` and degree. Node **opacity** follows recency, matching the decay
  recall uses.

### 5.2 Semantic zoom: categories dissolve into memories

Zoom is driven by Sigma's camera `ratio`. There are three bands with smooth transitions between them.
Each node and edge is rendered through `nodeReducer` / `edgeReducer`, which read a single `t` per
band (smoothstep over a ratio window), so the effect animates continuously with the scroll wheel.

| Band | What is drawn | Interaction |
|---|---|---|
| **0 — Categories** (zoomed out) | One disc per category: centre = members' centroid, area ∝ count, label + count + icon. Category edges, width ∝ weight. Member nodes hidden. | Click a category to fly into it. Hover to show its top sub-labels and 3 sample memories. |
| **1 — Sub-labels** | Each category disc splits into its sub-label clusters (or one cluster if it has none), drawn as soft convex hulls on a canvas layer behind the nodes. Member nodes appear as small dots without labels. | Click a cluster to fly in. Lasso-select a cluster for bulk edits. |
| **2 — Memories** (zoomed in) | Full nodes with truncated content labels (collision-culled), similarity edges, relation edges (dashed, predicate on hover). Hulls fade to a faint outline. | Click a memory to open the editor. Hover to highlight neighbours. Drag to pin. |

**The dissolve**
- Within a transition window, each member node is drawn at `lerp(categoryCentroid, ownPosition, t)`
  with alpha `t`.
- The category disc is drawn with alpha `1 − t`, and its radius grows towards the hull's.
- Collapsing on zoom-out is the same animation in reverse.
- A **level indicator** (Categories · Clusters · Memories) sits by the zoom controls; clicking a level
  jumps the camera to it.
- Keyboard: `+`/`-` to zoom, `0` to reset.

**Large stores** (above 5 000 nodes, §4.1): band 0 and band 1 come from aggregates. Entering band 2
over a category fetches that category's nodes and lays them out in place.

### 5.3 Encoding: never by colour alone

This follows the owner's accessibility rule. Every category gets:
- a colour from a categorical palette validated for both light and dark themes;
- **plus a glyph**: an icon inside the disc and a shape for its memory nodes;
- **plus its text label.**

Edge kinds differ by stroke style (solid for similarity, dashed for relation) as well as by colour.
Selected or highlighted states use a ring and weight, not only a hue.

### 5.4 Panels and controls

- **Search / recall preview** (G5):
  - The box calls `/v1/memories/search`. Hits light up on the map in rank order, and the camera fits
    them.
  - A side list shows relevance, score and which legs matched.
  - A "why" detail per hit (vector rank, text rank, graph rank) makes recall tuning visible.
- **Filters:** categories (multi), tags, date range, include superseded, edge kinds, similarity floor
  (slider, re-filters client-side over the edges it already has), k.
- **Memory editor** (side panel):
  - Fields: content (textarea), category (combobox with "create new…"), sub-label, tags (chip input),
    expiry.
  - Shown read-only: created, updated, confidence, source client, audit history.
  - Actions: Save (primary), Forget (secondary, with confirm and then the undo toast).
  - Follows the owner's form rules: visible labels, validate on blur and submit, specific errors, input
    never cleared on failure, one primary action.
- **Add memory:** the same form, plus the "Let the AI classify and de-duplicate it" toggle (D7).
  - With the toggle on, category and tags become optional hints, and the UI shows the job state until
    the pipeline reports which memories it created, merged or skipped.
  - The new node then appears and is highlighted.
- **List view:** a sortable, filterable table of the same data. It is the accessible alternative to
  the map, the efficient place for bulk selection, and the phone-width view.
- **Categories manager:** the table from §4.2 with counts. It offers rename, relabel, merge (pick
  target) and delete (pick a move target if non-empty). Each destructive action shows how many
  memories it touches, and whether they will be re-embedded, before it runs.
- **Scope-aware UI:** with a read-only key, every edit control is hidden, and a "read-only key" badge
  is shown.

## 6. Security

- The API key is held in `sessionStorage`. It is cleared on tab close and on sign-out, never written to
  `localStorage`, never put in a URL. It is sent only as `Authorization: Bearer`.
- Same-origin only. `connect-src 'self'` in the CSP blocks exfiltration to other hosts even if an XSS
  were found.
- Memory content is rendered as text, never as HTML: no `dangerouslySetInnerHTML`. Labels on the
  canvas are drawn as text by Sigma.
- The existing rate limiter covers the new endpoints. `/v1/map` is the most expensive read, so it gets
  its own lower per-key limit.
- `docs/security.md` gains a "Web UI" section: what is stored in the browser, CSP, and the `none`
  auth-mode warning.

## 7. Testing

TDD throughout, as the owner prefers: failing tests first, fakes over mocks, one behaviour per test,
Arrange-Act-Assert, and test names that describe behaviour.

| Layer | What | Tooling |
|---|---|---|
| Rust domain | `Taxonomy` resolve / rename / merge rules; mutual-kNN edge builder (hub suppression, floor from baseline); generation bumps | unit tests, in-memory fakes |
| Rust application | Bulk update re-embeds only label-affecting changes; restore re-indexes; rename of a custom category re-embeds members; `If-Match` conflict | existing `test_doubles.rs` fakes |
| Rust HTTP | Every new endpoint: scope gating, user isolation (extend `tests/identity_isolation.rs`), validation errors, 404/409/410/412 | integration tests via `tests/common` harness |
| Migration | V10 seeds from config extras and from existing memory categories; idempotent on re-run | `storage_tests.rs` style |
| Web domain | Zoom-band maths, cluster summaries, centroid/lerp, filter logic | Vitest, pure functions |
| Web components | Editor form rules (blur validation, no clearing on error), scope-aware hiding, undo toast | Vitest + Testing Library, in-memory API fake implementing the `api/` interface |
| End to end | Sign in with a key → map renders bands 0/1/2 → edit a memory → re-categorise → forget + undo → create & merge a category | Playwright against a daemon spawned by the same harness as the Rust integration tests |
| Recall safety | `eval --baseline eval/baseline.json` unchanged after the taxonomy migration (no ranking change expected) | existing eval |

Coverage targets as standing policy: 90 % for the domain layer (Rust and `web/src/domain`), 80 %
overall.

## 8. Build, CI and release

- `just web-build` runs `pnpm --dir web install --frozen-lockfile && pnpm --dir web build`.
  `just build` depends on it when the `ui` feature is on.
- The dev container (`docker/`) gains Node 22 + corepack pnpm.
- CI:
  - a new `web` job: lint (ESLint + `tsc --noEmit`), Vitest, build, then upload `web/dist` as an
    artifact;
  - the `test` job downloads that artifact before `cargo build`;
  - Playwright runs in its own job.
- `release.yml` builds `web/dist` once, then each target embeds it. The glibc-floor check is
  unaffected: assets are bytes, not linked code.
- Bundle budget: **≤ 400 KB gzipped** for the initial load. Sigma and graphology are about 120 KB; the
  map route is lazy-loaded so the list view starts faster. CI fails the build above the budget.
- The binary grows by the gzipped asset size. Recorded in `docs/performance.md`.

## 9. Phases

Each phase ends with its tests green, `docs/` updated, a CHANGELOG entry, and a PR.

| Phase | Deliverable | Acceptance |
|---|---|---|
| **P0 — Skeleton** | `web/` project, rust-embed under `ui` feature, `/ui` routes + CSP, login with API key via `/v1/ping`, scope badge, list view of memories (read-only) | `just build` produces one binary serving the UI; read key sees list; bad key shows a specific error; no-ui build needs no Node |
| **P1 — Manage memories** | Editor panel (edit, re-categorise, tag), add (direct + AI toggle with job polling), forget + `:restore` undo, `If-Match` | Every D7/D8 flow passes e2e; a concurrent MCP edit produces the 412 prompt |
| **P2 — The map** | `GET /v1/map` (mutual-kNN + relation + category edges, generation cache), Sigma map with the three zoom bands and the dissolve, search / recall preview, filters | 1 000-memory synthetic store: endpoint < 300 ms, map ≥ 50 fps while zooming on a mid laptop; real store's categories readable at band 0 |
| **P3 — Taxonomy** | V10 migration, `Taxonomy` port replacing config extras across HTTP / MCP / pipeline, categories manager, rename / relabel / merge / delete, `:bulk` with lasso selection on the map | Migration seeds correctly on a copy of the field store; eval baseline unchanged; renaming a custom category re-embeds its members and recall still finds them |
| **P4 — Polish** | Large-store aggregate mode (> 5 000), layout persistence, keyboard navigation, dark mode, phone-width list/editor, docs (`docs/web-ui.md`, screenshots) | Accessibility pass (keyboard-only walk-through; colour-blind simulation of the map) |

P1 comes before P2 on purpose: management delivers value on its own, and it exercises the auth and
API-client layers that the map then reuses. Doing P3 last keeps the riskiest backend change (the
taxonomy refactor) out of the way of the visible work. Until P3, the UI's category choices come from
`GET /v1/categories`, backed temporarily by the built-ins plus config extras.

### 9.1 What P0 shipped, and where it differs from the plan above

**Shipped:**
- `src/bootstrap/web_ui.rs`:
  - routes `/ui`, `/ui/` and `/ui/{*path}`;
  - SPA fallback for extension-less paths, 404 for missing files;
  - path-traversal refusal;
  - `immutable` caching for `assets/` and `no-cache` otherwise;
  - CSP, `nosniff`, `DENY` and `no-referrer` on every response.
- An `AssetSource` port, so tests serve an in-memory bundle.
- `[ui].enabled`.
- The rate-limit exemption for UI paths.
- `web/`:
  - an API client (the only module that calls `fetch`), with an in-memory fake;
  - pure domain modules (access, key shape, filter, sort, labels);
  - the session provider;
  - the sign-in screen, the header with the scope badge, and the list view.
- Tests:
  - 10 Rust unit tests;
  - 5 integration tests (`tests/web_ui.rs`);
  - 42 Vitest tests.
- CI `web` job, release and Docker builds of the bundle, docs (`docs/web-ui.md`).

**Differences, each deliberate:**
- **A missing bundle is a 503, not a build failure.** Failing the build would make Node a
  requirement for the test suite, the eval and the dev container, none of which open the UI. The
  release workflow and the Dockerfile build the bundle first; the release fails if it is missing.
- **No `[ui] path`.** The mount is fixed at `/ui` (see §3).
- **The list view reads `GET /v1/memories/export?format=json`**, which gained `subcategory`. A
  paginated list endpoint is not needed at personal scale; `GET /v1/map` (P2) supersedes it for the
  map.
- **UI asset requests are exempt from the rate limiter**, like `/healthz`. Loading the page fetches
  several files at once, and charging them would spend a browser's budget before its first API call.
- **ESLint is not set up yet.** `tsc --strict` (with `noUncheckedIndexedAccess` and
  `exactOptionalPropertyTypes`) and Vitest gate CI for now. ESLint joins in P4.
- **Playwright e2e is deferred to P1**, where there are flows worth driving. P0 was checked by hand
  against a live daemon: sign-in, list, deep-link reload, phone width, no CSP violations.

### 9.2 What P1 shipped, and where it differs from the plan above

**Shipped:**
- **Backend:**
  - `ETag` / `If-Match` / `412` on the single-memory routes (§4.5), implemented as
    `MemoryEdit::expected_updated_at`, checked by `MemoryUpdater`, and
    `RaError::PreconditionFailed`;
  - `MemoryRestorer` and `POST /v1/memories/{id}/restore` (§4.4);
  - `GET /v1/categories` (the read half of §4.2, ahead of P3).
- **Web:**
  - the editor panel, the add form (direct + AI), forget with confirm, and an
    8-second undo;
  - conflict resolution;
  - read-only viewing.
- **Tests:**
  - Rust: 6 restorer tests, 2 updater precondition tests, 4 handler tests, and 8
    integration tests (`tests/memory_editing.rs`);
  - Web: 79 Vitest tests against a `FakeServer` that enforces scopes, isolation,
    versions and restore rules.
- **Checked by hand against a live daemon:** edit, a real 412 provoked by a
  concurrent `PATCH`, load latest, forget → undo (recalled again at relevance
  0.66), and AI add on a daemon without a model.

**Differences, each deliberate:**
- **`/restore` is a sub-path, not `:restore`.** axum's router does not match a
  parameter with a suffix inside one segment, and `/v1/memories/{id}/restore`
  reads as plainly.
- **The AI add uses `POST /v1/memories` with `wait: true`, not job polling.** The
  synchronous answer carries the `outcome` (stored / already known / nothing
  durable / changed), which the job record does not. The 180 s ingest timeout
  bounds the wait, and the button says "Asking the AI…" throughout.
- **`GET /v1/categories` ships in P1, not P3.** The editor needs labels and valid
  ids now. It is backed by built-ins + config extras + stored leftovers until the
  taxonomy table lands.
- **The audit actor for UI edits is still `rest`.** The UI sends
  `X-Recuerdos-Client: web-ui`, and direct saves record `web-ui` as their
  source client, but the REST handlers do not yet read the header for the actor.
  This is a small follow-up, not needed for any P1 behaviour.
- **Playwright e2e is still deferred, now to P2.** The component tests drive the
  real `App` through every flow against an enforcing fake, and the live check
  above covered the wire. The map is where a real browser earns its keep, since
  WebGL cannot run in jsdom.

## 10. Risks and open points

| Risk | Mitigation |
|---|---|
| Similarity edges form a hairball, or a star around hub memories | Mutual kNN, a floor above the model baseline, a client-side floor slider; measure edge count per node on the field store in P2 before tuning k |
| The map looks arbitrary because categories are uneven (one huge `preference.coding`, several tiny ones) | Band 1 sub-label clusters split the big category; area ∝ √count rather than count |
| The taxonomy refactor touches recall's category filter and the classifier prompt | Separate phase (P3); eval baseline check; seed migration keeps every existing category id valid |
| Re-embedding on custom-category rename is slow with a remote embedder | Threshold to background job; UI shows progress; built-in relabel and display-label changes never re-embed |
| Layout jumps after every edit | Deterministic seeds, cached positions, warm re-layout of touched nodes only |
| API key in the browser | `sessionStorage` only, strict CSP, same origin; a session-cookie exchange remains a possible later upgrade if keys in the browser become a concern |
| Node toolchain becomes required to build | `ui` feature can be disabled; CI builds assets once; dev container ships pnpm |

**Open points to settle during implementation (not blocking the plan):**

1. Whether band 1 should cluster by **sub-label** (the stored field: explainable, but sparse on some
   categories) or by **similarity communities** inside the category (Louvain over the similarity
   edges: richer, but unnamed). The suggestion is sub-label when at least 60 % of a category's members
   have one, Louvain otherwise. Measure on the field store in P2.
2. Whether the recall-preview panel should also expose per-request knobs (limit, `min_relevance`), so
   the UI doubles as a tuning tool.
3. Purging forgotten memories: out of scope here. If a purge ever exists, `:restore` returns 410 for
   purged rows, as specified.
