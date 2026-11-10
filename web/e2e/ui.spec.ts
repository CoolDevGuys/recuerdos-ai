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

  const map = page.getByRole("img", { name: /Map of \d+ memories/ });
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
