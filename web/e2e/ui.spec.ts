import { expect, test, type Page } from "@playwright/test";

/**
 * The web UI against a real daemon: real HTTP, real embeddings, real
 * Sigma/WebGL. Each run seeds its own memories under a run-unique word so
 * it can share a store with earlier runs.
 */
const KEY = process.env.RECUERDOS_KEY ?? "";
const RUN = `r${Date.now().toString(36)}`;

// The structured save shape (ai-specs/memory-structure-plan.md §3). The
// category is still accepted alongside, so the map groups as before.
const preference = { kind: "preference", topic: "coding", applies_to: { target: "me" }, origin: "user_said" };
const general = (kind: string) => ({ kind, topic: "coding", applies_to: { target: "general" }, origin: "user_said" });
const SEED = [
  { content: `Unit tests are written with Vitest (${RUN})`, category: "preference.coding", subtopic: "testing", tags: ["testing"], ...preference },
  { content: `Tests come before the code, test-driven development (${RUN})`, category: "preference.coding", subtopic: "testing", tags: ["testing"], ...preference },
  { content: `The team deploys the backend on Hetzner with Docker (${RUN})`, category: "fact.project", tags: ["deploy"], ...general("fact") },
  { content: `We chose SQLite over Postgres for a single binary (${RUN})`, category: "decision", tags: ["database"], ...general("decision") },
];

test.beforeAll(async ({ request }) => {
  test.skip(KEY === "", "set RECUERDOS_KEY to a fresh read,write key");
  for (const memory of SEED) {
    const response = await request.post("/v1/memories:direct", {
      headers: { Authorization: `Bearer ${KEY}` },
      data: memory,
    });
    expect(response.status()).toBe(201);
  }
});

async function signIn(page: Page, path = "/ui/") {
  const violations: string[] = [];
  page.on("console", (message) => {
    if (message.text().includes("Content Security Policy")) violations.push(message.text());
  });
  await page.goto(path);
  await page.getByLabel("API key").fill(KEY);
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByText(/Read & write key|Admin key/)).toBeVisible();
  return violations;
}

test("signs in and lists the memories", async ({ page }) => {
  const violations = await signIn(page);

  await page.getByLabel("Filter").fill(RUN);
  await expect(page.getByRole("listitem")).toHaveCount(SEED.length);
  expect(violations).toEqual([]);
});

test("corrects a memory, then forgets it and takes it back", async ({ page }) => {
  await signIn(page);
  await page.getByLabel("Filter").fill(`Hetzner with Docker (${RUN})`);
  await page.getByRole("button", { name: /deploys the backend on Hetzner/ }).click();

  const editor = page.getByRole("dialog", { name: "Edit memory" });
  await editor.getByLabel("Memory").fill(`The team deploys the backend on Hetzner with Docker Compose (${RUN})`);
  await editor.getByRole("button", { name: "Save changes" }).click();
  await expect(page.getByText("Changes saved.")).toBeVisible();

  await page.getByLabel("Filter").fill(`Docker Compose (${RUN})`);
  await page.getByRole("button", { name: /Docker Compose/ }).click();
  await editor.getByRole("button", { name: "Forget…" }).click();
  await editor.getByRole("button", { name: "Forget", exact: true }).click();
  await expect(page.getByText("Memory forgotten.")).toBeVisible();
  await expect(page.getByText("No memory matches this filter.")).toBeVisible();

  await page.getByRole("button", { name: "Undo" }).click();
  await expect(page.getByText("Memory restored.")).toBeVisible();
  await expect(page.getByRole("button", { name: /Docker Compose/ })).toBeVisible();
});

test("draws the map and zooms through its levels", async ({ page }) => {
  const violations = await signIn(page, "/ui/map");

  const map = page.getByRole("application", { name: /Map of \d+ memories/ });
  await expect(map).toBeVisible();
  // Sigma draws into canvases inside the container.
  await expect(map.locator("canvas").first()).toBeAttached();
  const levels = page.getByRole("group", { name: "Zoom level" });
  await expect(levels.getByRole("button", { name: "Topics" })).toHaveAttribute("aria-pressed", "true");

  await levels.getByRole("button", { name: "Memories" }).click();
  // Pressed only once the real camera has arrived and reported the level.
  await expect(levels.getByRole("button", { name: "Memories" })).toHaveAttribute("aria-pressed", "true");
  await expect(map).toHaveAccessibleName(/showing memories/);

  await page.getByRole("button", { name: "Fit" }).click();
  await expect(levels.getByRole("button", { name: "Topics" })).toHaveAttribute("aria-pressed", "true");
  expect(violations).toEqual([]);
});

test("previews recall on the map and opens a result", async ({ page }) => {
  await signIn(page, "/ui/map");

  await page.getByLabel("Ask your memories").fill(`how are unit tests written ${RUN}`);
  await page.getByRole("button", { name: "Recall" }).click();

  const first = page.getByRole("listitem").filter({ hasText: /^1\./ });
  await expect(first).toContainText(RUN);
  await expect(first).toContainText(/% relevant/);
  await first.getByRole("button").click();
  await expect(page.getByRole("dialog", { name: "Edit memory" })).toBeVisible();
});

test("creates a topic and bulk-files memories under it, and recall filters by it", async ({ page, request }) => {
  const topic = `t${RUN}`;
  await signIn(page, "/ui/topics");
  await page.getByLabel("New topic").fill(topic);
  await page.getByLabel("Label (optional)").fill(`Topic ${RUN}`);
  await page.getByRole("button", { name: "Add topic" }).click();
  await expect(page.getByText(`Added the topic Topic ${RUN}.`)).toBeVisible();

  // Select the two testing memories in the list and re-file them in one request.
  await page.getByRole("link", { name: "List" }).click();
  await page.getByLabel("Filter").fill(`(${RUN})`);
  await page.getByRole("button", { name: "Select" }).click();
  await page.getByRole("checkbox", { name: /written with Vitest/ }).check();
  await page.getByRole("checkbox", { name: /test-driven development/ }).check();
  const editor = page.getByRole("region", { name: "2 memories selected" });
  await editor.getByLabel("About").selectOption(topic);
  await editor.getByRole("button", { name: "Apply to 2 memories" }).click();
  await expect(page.getByText("Updated 2 memories.")).toBeVisible();

  const recalled = await request.post("/v1/memories/search", {
    headers: { Authorization: `Bearer ${KEY}` },
    data: { query: "how are tests written", topics: [topic] },
  });
  expect((await recalled.json()).results).toHaveLength(2);
});

test("files a memory from the Needs review queue", async ({ page, request }) => {
  // Saved the old way, with no model: filed by a guess, flagged for review.
  const content = `The NAS backups are in /mnt/tank (${RUN})`;
  const saved = await request.post("/v1/memories", {
    headers: { Authorization: `Bearer ${KEY}` },
    data: { content, category: "fact.project", wait: true },
  });
  expect(saved.status()).toBe(201);

  await signIn(page, "/ui/review");
  await expect(page.getByRole("heading", { name: /Needs review/ })).toBeVisible();
  await page.getByRole("checkbox", { name: `Select: ${content}` }).check();
  const editor = page.getByRole("region", { name: "1 memory selected" });
  await editor.getByLabel("What they are").selectOption("reference");
  await editor.getByLabel("About").selectOption("tech");
  await editor.getByLabel("Applies to").selectOption("general");
  await editor.getByRole("button", { name: "Apply to 1 memory" }).click();
  await expect(page.getByText("Updated 1 memory.")).toBeVisible();
  await expect(page.getByRole("checkbox", { name: `Select: ${content}` })).toHaveCount(0);

  const id = (await saved.json()).memory_ids[0];
  const memory = await (await request.get(`/v1/memories/${id}`, { headers: { Authorization: `Bearer ${KEY}` } })).json();
  expect(memory).toMatchObject({ kind: "reference", topic: "tech", needs_review: false });
});

test("says what the daemon is, learns your names and files a memory about you as yours", async ({ page, request }) => {
  // As v0.6.0's upgrade left an old "fact about a person": general.
  const name = `Zed${RUN} Quinn`;
  const content = `${name} is a backend engineer in Berlin`;
  const saved = await request.post("/v1/memories", {
    headers: { Authorization: `Bearer ${KEY}` },
    data: { content, category: "fact.person", wait: true },
  });
  expect(saved.status()).toBe(201);

  await signIn(page, "/ui/about");
  await expect(page.getByRole("heading", { name: /^Recuerdos AI \d+\.\d+\.\d+/ })).toBeVisible();
  await page.getByLabel("Your name").fill(name);
  await page.getByRole("button", { name: "Save names" }).click();
  await expect(page.getByText(/1 memory about you was filed as someone else's/)).toBeVisible();

  await page.getByRole("link", { name: /Needs review/ }).click();
  const row = page.getByRole("listitem").filter({ hasText: content });
  await row.getByRole("button", { name: "It’s about me" }).click();
  await expect(page.getByText("Filed as your personal information.")).toBeVisible();

  const id = (await saved.json()).memory_ids[0];
  const memory = await (await request.get(`/v1/memories/${id}`, { headers: { Authorization: `Bearer ${KEY}` } })).json();
  expect(memory).toMatchObject({ kind: "fact", applies_to: { target: "me" }, needs_review: false });
  const profile = await (await request.get("/v1/profile", { headers: { Authorization: `Bearer ${KEY}` } })).text();
  expect(profile).toContain(`The user is ${name}.`);
});

test("renames a project, and its memories follow", async ({ page, request }) => {
  const project = `${RUN}-quill`;
  const saved = await request.post("/v1/memories:direct", {
    headers: { Authorization: `Bearer ${KEY}` },
    data: {
      content: `Use cases are named by intent (${RUN})`,
      kind: "convention",
      topic: "coding",
      applies_to: { target: "project", project_name: project, new_project: true },
      origin: "user_said",
    },
  });
  expect(saved.status()).toBe(201);

  await signIn(page, "/ui/projects");
  const row = page.getByRole("listitem").filter({ has: page.getByRole("heading", { name: project }) });
  await row.getByRole("button", { name: "Rename…" }).click();
  await row.getByLabel(`New name for ${project}`).fill(`${RUN}-app`);
  await row.getByRole("button", { name: "Rename", exact: true }).click();
  await expect(page.getByText(new RegExp(`Renamed ${project} to ${RUN}-app`))).toBeVisible();

  const renamed = page.getByRole("listitem").filter({ has: page.getByRole("heading", { name: `${RUN}-app` }) });
  await expect(renamed).toContainText(`Also found by ${project}`);
  await renamed.getByRole("button", { name: "Show its memories" }).click();
  await expect(page.getByRole("button", { name: /Use cases are named by intent/ })).toBeVisible();
  await expect(page.getByText(`Convention · Coding · ${RUN}-app`)).toBeVisible();
});

test("closes a goal: it leaves the current list and shows under Closed goals", async ({ page, request }) => {
  const content = `Run a half marathon in April (${RUN})`;
  const saved = await request.post("/v1/memories:direct", {
    headers: { Authorization: `Bearer ${KEY}` },
    data: { content, kind: "goal", topic: "health", applies_to: { target: "me" }, origin: "user_said" },
  });
  expect(saved.status()).toBe(201);

  await signIn(page);
  await page.getByLabel("Filter").fill(RUN);
  await page.getByRole("button", { name: new RegExp(`half marathon in April`) }).click();
  const editor = page.getByRole("dialog", { name: "Edit memory" });
  await expect(editor.getByText(/Open: you are working towards it/)).toBeVisible();
  await editor.getByRole("button", { name: "Mark achieved" }).click();
  await expect(page.getByText("Changes saved.")).toBeVisible();

  await expect(page.getByRole("button", { name: /half marathon in April/ })).toHaveCount(0);
  await page.getByLabel("Show").selectOption("closed");
  await expect(page.getByRole("button", { name: /half marathon in April/ })).toBeVisible();
  await expect(page.getByText("Goal achieved")).toBeVisible();
});

test("shows a conflict to the user, who keeps both", async ({ page, request }) => {
  // Without a model, a near-repeat of a stored memory is the conflict caught.
  const stored = await request.post("/v1/memories:direct", {
    headers: { Authorization: `Bearer ${KEY}` },
    data: { content: `The user prefers tabs for indentation (${RUN})`, ...preference },
  });
  expect(stored.status()).toBe(201);

  await signIn(page);
  await page.getByRole("button", { name: "+ Add memory" }).click();
  const panel = page.getByRole("dialog", { name: "Add a memory" });
  await panel.getByLabel("Memory").fill(`The user prefers spaces for indentation (${RUN})`);
  await panel.getByLabel("What it is").selectOption("preference");
  await panel.getByLabel("About").selectOption("coding");
  await panel.getByRole("radio", { name: /Me, everywhere/ }).check();
  await panel.getByRole("button", { name: "Save memory" }).click();

  await expect(panel.getByText("This may contradict a memory you already have:")).toBeVisible();
  await expect(panel.getByText(`The user prefers tabs for indentation (${RUN})`)).toBeVisible();
  await panel.getByRole("button", { name: "Keep both" }).click();
  await expect(page.getByText("Memory saved.")).toBeVisible();
});

test("lays the map out in a worker, keeps the layout, and answers the keyboard", async ({ page }) => {
  const violations = await signIn(page, "/ui/map");
  const map = page.getByRole("application", { name: /Map of/ });
  await expect(map).toBeVisible();

  // The layout ran off the main thread, under the CSP.
  const workers = await page.evaluate(() =>
    performance.getEntriesByType("resource").filter((e) => e.name.includes("layout.worker")).length,
  );
  expect(workers).toBeGreaterThan(0);
  await expect
    .poll(() => page.evaluate(() => Object.keys(localStorage).some((k) => k.startsWith("recuerdos.map-layout.v1:"))))
    .toBe(true);

  await map.focus();
  await page.keyboard.press("3");
  await expect(page.getByRole("button", { name: "Memories" })).toHaveAttribute("aria-pressed", "true");
  await page.keyboard.press("0");
  await expect(page.getByRole("group", { name: "Zoom level" }).getByRole("button", { name: "Topics" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  expect(violations).toEqual([]);
});
