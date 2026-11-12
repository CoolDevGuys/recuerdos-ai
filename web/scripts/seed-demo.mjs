// Fills a fresh user's store with made-up memories, for screenshots and
// trying the UI. Nothing here is about a real person.
//
//   RECUERDOS_URL=http://127.0.0.1:7070 RECUERDOS_KEY=ra_live_… node scripts/seed-demo.mjs
//
// Use a key for a new, empty user: the script only adds.
const base = process.env.RECUERDOS_URL ?? "http://127.0.0.1:7070";
const key = process.env.RECUERDOS_KEY;
if (!key) {
  console.error("Set RECUERDOS_KEY to a read,write key for an empty user.");
  process.exit(1);
}

const coding = (content, subcategory, tags) => ({ content, category: "preference.coding", subcategory, tags });
const memories = [
  coding("The user writes unit tests with Vitest", "testing", ["testing", "vitest"]),
  coding("Tests come first: red, green, refactor", "testing", ["testing", "tdd"]),
  coding("End-to-end tests run in Playwright against a real server", "testing", ["testing", "playwright"]),
  coding("Snapshot tests are avoided; assertions name the behaviour", "testing", ["testing"]),
  coding("pnpm is the package manager for every JavaScript project", null, ["tooling", "pnpm"]),
  coding("Formatting is left to the formatter, never argued in review", null, ["tooling"]),
  coding("Prefers small pull requests that each ship something", null, ["workflow"]),
  coding("Commits follow Conventional Commits", null, ["workflow", "git"]),
  coding("Rebases feature branches rather than merging main into them", null, ["workflow", "git"]),
  coding("TypeScript runs in strict mode with noUncheckedIndexedAccess", null, ["typescript"]),
  coding("Avoids default exports in TypeScript modules", null, ["typescript"]),
  coding("Prefers Rust for long-running services", null, ["rust", "backend"]),
  coding("Errors in Rust services use thiserror at the edges", null, ["rust", "backend"]),
  coding("SQL migrations are forward-only and numbered", null, ["backend", "database"]),
  coding("Every form field has a visible label", null, ["accessibility", "frontend"]),
  coding("Never encodes meaning by colour alone", null, ["accessibility", "frontend"]),
  coding("Components are tested through what the user sees, not their internals", null, ["frontend", "testing"]),
  coding("Tailwind for styling, no CSS-in-JS", null, ["frontend"]),
  { content: "Enjoys trail running at weekends", category: "preference.personal", tags: ["sport"] },
  { content: "Drinks green tea, not coffee", category: "preference.personal", tags: ["food"] },
  { content: "Prefers written updates over meetings", category: "preference.personal", tags: ["work"] },
  { content: "Reads science fiction in the evenings", category: "preference.personal", tags: ["books"] },
  { content: "Chose SQLite with WAL for the notes service", category: "decision", tags: ["database"] },
  { content: "Decided to self-host on a single VPS instead of Kubernetes", category: "decision", tags: ["infra"] },
  { content: "Picked Sigma.js for graph drawing because it renders with WebGL", category: "decision", tags: ["frontend"] },
  { content: "Dropped GraphQL for plain REST endpoints", category: "decision", tags: ["backend", "api"] },
  { content: "Releases are tagged from main and built by CI only", category: "decision", tags: ["workflow"] },
  { content: "The notes service is called Quill and lives in the monorepo", category: "fact.project", tags: ["quill"] },
  { content: "Quill's API listens on port 8080 behind Caddy", category: "fact.project", tags: ["quill", "infra"] },
  { content: "Quill stores attachments in an S3-compatible bucket", category: "fact.project", tags: ["quill", "storage"] },
  { content: "The staging environment resets every night at 02:00", category: "fact.project", tags: ["infra"] },
  { content: "The design system is called Fern and ships as a package", category: "fact.project", tags: ["fern", "frontend"] },
  { content: "Fern's tokens are generated from a single JSON file", category: "fact.project", tags: ["fern"] },
  { content: "Maya leads the design system team", category: "fact.person", tags: ["fern", "team"] },
  { content: "Jonas owns the deployment pipeline", category: "fact.person", tags: ["infra", "team"] },
  { content: "Priya reviews every database migration", category: "fact.person", tags: ["database", "team"] },
  { content: "Migrated Quill from Postgres to SQLite in a weekend", category: "experience", tags: ["database", "quill"] },
  { content: "A missing index once made the search page take nine seconds", category: "experience", tags: ["database"] },
  { content: "Learned that flaky tests usually hide a shared clock", category: "experience", tags: ["testing"] },
  { content: "Ran the first public demo of Fern at a meetup", category: "experience", tags: ["fern"] },
  { content: "Comfortable with Rust, TypeScript and SQL", category: "skill", tags: ["rust", "typescript"] },
  { content: "Can set up Caddy, systemd units and backups on a VPS", category: "skill", tags: ["infra"] },
  { content: "Knows WCAG 2.2 well enough to audit a page", category: "skill", tags: ["accessibility"] },
  { content: "The Rust API guidelines are the reference for public APIs", category: "reference", tags: ["rust"] },
  { content: "WAI-ARIA Authoring Practices for widget keyboard patterns", category: "reference", tags: ["accessibility"] },
  { content: "Sleeps best with seven and a half hours", category: "health.sleep", tags: ["sleep"] },
  { content: "No screens after 23:00 helps falling asleep", category: "health.sleep", tags: ["sleep"] },
];

async function call(method, path, body) {
  const response = await fetch(`${base}${path}`, {
    method,
    headers: { Authorization: `Bearer ${key}`, "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!response.ok && response.status !== 409) {
    throw new Error(`${method} ${path}: ${response.status} ${await response.text()}`);
  }
  return response;
}

// The structured save shape (ai-specs/memory-structure-plan.md §3): what each
// memory is, what it is about and who it is true for. The category is kept so
// the map groups the demo the same way.
const STRUCTURE = {
  "preference.coding": ["preference", "coding", "me"],
  "preference.personal": ["preference", "lifestyle", "me"],
  decision: ["decision", "work", "general"],
  "fact.project": ["fact", "coding", "general"],
  "fact.person": ["fact", "people", "general"],
  experience: ["experience", "work", "general"],
  skill: ["procedure", "tech", "general"],
  reference: ["reference", "coding", "general"],
  "health.sleep": ["fact", "health", "general"],
};

await call("POST", "/v1/categories", { id: "health.sleep", label: "Sleep" });
for (const { subcategory, ...memory } of memories) {
  const [kind, topic, target] = STRUCTURE[memory.category];
  await call("POST", "/v1/memories:direct", {
    ...memory,
    ...(subcategory != null && { subtopic: subcategory }),
    kind,
    topic,
    applies_to: { target },
    origin: "user_said",
  });
}
console.log(`Saved ${memories.length} memories.`);
