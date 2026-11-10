import { expect, test, type Page } from "@playwright/test";

/**
 * The web UI against a real daemon: real HTTP, real embeddings, real
 * Sigma/WebGL. Each run seeds its own memories under a run-unique word so
 * it can share a store with earlier runs.
 */
const KEY = process.env.RECUERDOS_KEY ?? "";
const RUN = `r${Date.now().toString(36)}`;

const SEED = [
  { content: `Unit tests are written with Vitest (${RUN})`, category: "preference.coding", subcategory: "testing", tags: ["testing"] },
  { content: `Tests come before the code, test-driven development (${RUN})`, category: "preference.coding", subcategory: "testing", tags: ["testing"] },
  { content: `The team deploys the backend on Hetzner with Docker (${RUN})`, category: "fact.project", tags: ["deploy"] },
  { content: `We chose SQLite over Postgres for a single binary (${RUN})`, category: "decision", tags: ["database"] },
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
  await expect(levels.getByRole("button", { name: "Categories" })).toHaveAttribute("aria-pressed", "true");

  await levels.getByRole("button", { name: "Memories" }).click();
  // Pressed only once the real camera has arrived and reported the level.
  await expect(levels.getByRole("button", { name: "Memories" })).toHaveAttribute("aria-pressed", "true");
  await expect(map).toHaveAccessibleName(/showing memories/);

  await page.getByRole("button", { name: "Fit" }).click();
  await expect(levels.getByRole("button", { name: "Categories" })).toHaveAttribute("aria-pressed", "true");
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

test("creates a category, bulk-moves memories into it, renames it and merges it away", async ({ page, request }) => {
  const id = `e2e.${RUN}`;
  await signIn(page, "/ui/categories");
  await page.getByLabel("Id").fill(id);
  await page.getByRole("button", { name: "Create" }).click();
  await expect(page.getByRole("row", { name: new RegExp(id.replace(".", "\\.")) })).toBeVisible();

  // Select the two testing memories in the list and re-file them in one request.
  await page.getByRole("link", { name: "List" }).click();
  await page.getByLabel("Filter").fill(`(${RUN})`);
  await page.getByRole("button", { name: "Select" }).click();
  await page.getByRole("checkbox", { name: /written with Vitest/ }).check();
  await page.getByRole("checkbox", { name: /test-driven development/ }).check();
  const editor = page.getByRole("region", { name: "2 memories selected" });
  await editor.getByLabel("Move to category").selectOption(id);
  await editor.getByRole("button", { name: "Apply to 2 memories" }).click();
  await expect(page.getByText("Updated 2 memories.")).toBeVisible();

  // Rename: the members move with it and recall, filtered to the new id, still finds them.
  const renamed = `${id}.renamed`;
  await page.getByRole("link", { name: "Categories" }).click();
  await page.getByRole("button", { name: `Edit ${id.replace(/[._-]/g, " ").replace(/^./, (c) => c.toUpperCase())}` }).click();
  const panel = page.getByRole("dialog", { name: "Edit category" });
  await panel.getByLabel("Id").fill(renamed);
  await panel.getByRole("button", { name: "Save" }).click();
  await expect(page.getByText(`Renamed to ${renamed}; 2 memories moved.`)).toBeVisible();
  const recalled = await request.post("/v1/memories/search", {
    headers: { Authorization: `Bearer ${KEY}` },
    data: { query: "how are tests written", categories: [renamed] },
  });
  expect((await recalled.json()).results).toHaveLength(2);

  // Merge it back, leaving the store as the other tests expect.
  const merged = await request.post(`/v1/categories/${renamed}/merge`, {
    headers: { Authorization: `Bearer ${KEY}` },
    data: { into: "preference.coding" },
  });
  expect((await merged.json()).moved).toBe(2);
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
  await expect(page.getByRole("button", { name: "Categories" })).toHaveAttribute("aria-pressed", "true");
  expect(violations).toEqual([]);
});
