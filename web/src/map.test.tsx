import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import { FakeServer } from "./api/fake";
import { memoryKeyStore } from "./state/keyStore";
import { FakeRenderer } from "./test/fakeRenderer";
import { ALEX_KEY, renderApp, standardServer } from "./test/renderApp";

beforeEach(() => window.history.pushState(null, "", "/ui/"));

async function openMap(server: FakeServer = standardServer()) {
  const renderer = new FakeRenderer();
  renderApp({ server, keyStore: memoryKeyStore(ALEX_KEY), renderer });
  const user = userEvent.setup();
  await user.click(await screen.findByRole("link", { name: "Map" }));
  await screen.findByRole("application", { name: /Map of 3 memories/ });
  await waitFor(() => expect(renderer.model).not.toBeNull());
  return { server, renderer, user };
}

describe("the map view", () => {
  it("is its own URL and lays out every memory", async () => {
    const { renderer } = await openMap();

    expect(window.location.pathname).toBe("/ui/map");
    expect(screen.getByRole("link", { name: "Map" })).toHaveAttribute("aria-current", "page");
    expect(renderer.model?.memories.map((m) => m.id).sort()).toEqual(["a", "b", "c"]);
    expect(renderer.model?.categories.map((c) => [c.category, c.count])).toEqual([
      ["preference.coding", 2],
      ["fact.project", 1],
    ]);
  });

  it("opens straight on the map from its URL", async () => {
    window.history.pushState(null, "", "/ui/map");
    const renderer = new FakeRenderer();
    renderApp({ keyStore: memoryKeyStore(ALEX_KEY), renderer });

    expect(await screen.findByRole("application", { name: /Map of 3 memories/ })).toBeInTheDocument();
  });

  it("lists each category with its glyph, label and count, and filters it out on click", async () => {
    const { renderer, user } = await openMap();
    const legend = screen.getByRole("heading", { name: "Categories" }).parentElement!;
    const coding = within(legend).getByRole("button", { name: /Coding preference/ });
    expect(coding).toHaveTextContent("2");
    expect(coding).toHaveAttribute("aria-pressed", "true");

    await user.click(coding);

    expect(coding).toHaveAttribute("aria-pressed", "false");
    expect(renderer.lastView?.hiddenCategories.has("preference.coding")).toBe(true);
  });

  it("zooms to a level on request and shows which level the camera is on", async () => {
    const { renderer, user } = await openMap();
    const levels = screen.getByRole("group", { name: "Zoom level" });

    await user.click(within(levels).getByRole("button", { name: "Memories" }));
    expect(renderer.zooms).toEqual(["memories"]);

    renderer.reachLevel("clusters");
    await waitFor(() => expect(within(levels).getByRole("button", { name: "Clusters" })).toHaveAttribute("aria-pressed", "true"));
    expect(screen.getByRole("application")).toHaveAccessibleName(/showing clusters/);

    await user.click(screen.getByRole("button", { name: "Zoom in" }));
    await user.click(screen.getByRole("button", { name: "Fit" }));
    expect(renderer.zooms).toEqual(["memories", "in", "reset"]);
  });

  it("opens the editor when a memory on the map is clicked", async () => {
    const { renderer } = await openMap();

    renderer.clickMemory("b");

    const dialog = await screen.findByRole("dialog", { name: "Edit memory" });
    expect(within(dialog).getByLabelText("Memory")).toHaveValue("Testing follows TDD");
  });

  it("redraws after an edit", async () => {
    const { renderer, user } = await openMap();
    const before = renderer.model;
    renderer.clickMemory("c");
    const dialog = await screen.findByRole("dialog", { name: "Edit memory" });

    await user.selectOptions(within(dialog).getByLabelText("Category"), "Decision");
    await user.click(within(dialog).getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(renderer.model).not.toBe(before));
    expect(renderer.model?.byId.get("c")?.category).toBe("decision");
  });

  it("offers the relations toggle only when there are relations", async () => {
    await openMap();
    expect(screen.queryByLabelText(/Stated relations/)).not.toBeInTheDocument();
  });

  it("filters links by similarity and kind", async () => {
    const server = standardServer();
    server.mapEdges = [
      { kind: "similar", source: "a", target: "b", weight: 0.4 },
      { kind: "relation", source: "a", target: "c", predicate: "uses", entity: "pnpm" },
    ];
    const { renderer, user } = await openMap(server);
    expect(screen.getByText(/1 similarity links · 1 stated relations/)).toBeInTheDocument();

    await user.click(screen.getByLabelText(/Stated relations/));
    expect(renderer.lastView?.showRelations).toBe(false);

    fireEvent.change(screen.getByLabelText(/Minimum similarity/), { target: { value: "60" } });
    expect(renderer.lastView?.minWeight).toBe(0.6);
    expect(screen.getByLabelText(/Minimum similarity: 60%/)).toBeInTheDocument();
  });

  it("says so when there is nothing to map", async () => {
    const renderer = new FakeRenderer();
    renderApp({
      server: new FakeServer().addAccount(ALEX_KEY, { user: "alex", scopes: ["read", "write"] }),
      keyStore: memoryKeyStore(ALEX_KEY),
      renderer,
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("link", { name: "Map" }));

    expect(await screen.findByText(/nothing to map/)).toBeInTheDocument();
    expect(renderer.model).toBeNull();
  });

  it("goes back to the list", async () => {
    const { user } = await openMap();

    await user.click(screen.getByRole("link", { name: "List" }));

    expect(window.location.pathname).toBe("/ui/");
    expect(await screen.findByLabelText("Filter")).toBeInTheDocument();
  });
});

describe("recall preview", () => {
  it("shows what recall returns, highlights it on the map and focuses the camera", async () => {
    const { renderer, user } = await openMap();

    await user.type(screen.getByLabelText("Ask your memories"), "which package manager does the user prefer");
    await user.click(screen.getByRole("button", { name: "Recall" }));

    expect(await screen.findByText(/1\. The user prefers pnpm/)).toBeInTheDocument();
    expect(screen.getByText(/% relevant · Coding preference · meaning #1 · words #1/)).toBeInTheDocument();
    expect([...(renderer.lastView?.highlighted ?? [])]).toEqual(["a"]);
    expect(renderer.focused).toEqual([["a"]]);
  });

  it("opens a returned memory and clears the highlight", async () => {
    const { renderer, user } = await openMap();
    await user.type(screen.getByLabelText("Ask your memories"), "pnpm");
    await user.click(screen.getByRole("button", { name: "Recall" }));

    await user.click(await screen.findByRole("button", { name: /The user prefers pnpm/ }));
    expect(await screen.findByRole("dialog", { name: "Edit memory" })).toBeInTheDocument();
    await user.keyboard("{Escape}");

    await user.click(screen.getByRole("button", { name: "Clear" }));
    expect(renderer.lastView?.highlighted).toBeNull();
  });

  it("says when recall returns nothing, and asks for a question first", async () => {
    const { user } = await openMap();

    await user.click(screen.getByRole("button", { name: "Recall" }));
    expect(screen.getByText(/Type a question/)).toBeInTheDocument();

    await user.type(screen.getByLabelText("Ask your memories"), "kubernetes clusters");
    await user.click(screen.getByRole("button", { name: "Recall" }));
    expect(await screen.findByText("Recall returns nothing for this.")).toBeInTheDocument();
  });
});

describe("the map from the keyboard", () => {
  it("zooms, fits, jumps to a level and pans with keys once focused", async () => {
    const { renderer, user } = await openMap();

    screen.getByRole("application", { name: /Map of 3 memories/ }).focus();
    await user.keyboard("+-0");
    await user.keyboard("3");
    await user.keyboard("{ArrowRight}{ArrowUp}");

    expect(renderer.zooms).toEqual(["in", "out", "reset", "memories"]);
    expect(renderer.pans).toEqual([
      [0.2, 0],
      [0, -0.2],
    ]);
  });

  it("explains the keys next to the map", async () => {
    await openMap();

    expect(screen.getByRole("application", { name: /Map of 3 memories/ })).toHaveAccessibleDescription(/arrow keys pan/);
  });

  it("leaves lasso mode and clears a selection on Escape", async () => {
    const { renderer, user } = await openMap();
    await user.click(screen.getByRole("button", { name: "Lasso select" }));
    renderer.lasso(["a"]);
    await screen.findByRole("region", { name: "1 memory selected" });

    screen.getByRole("application", { name: /Map of 3 memories/ }).focus();
    await user.keyboard("{Escape}");

    expect(screen.queryByRole("region", { name: /selected/ })).not.toBeInTheDocument();
    expect(renderer.lassoModes.at(-1)).toBe(false);
  });
});

describe("a store above the detail limit", () => {
  function bigServer() {
    const server = standardServer();
    server.mapDetailLimit = 2;
    return server;
  }

  async function openAggregate() {
    const renderer = new FakeRenderer();
    const server = bigServer();
    renderApp({ server, keyStore: memoryKeyStore(ALEX_KEY), renderer });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("link", { name: "Map" }));
    await screen.findByRole("application", { name: /showing categories only/ });
    await waitFor(() => expect(renderer.model?.aggregate).toBe(true));
    return { server, renderer, user };
  }

  it("shows the categories and says why", async () => {
    const { renderer } = await openAggregate();

    expect(screen.getByText(/You have 3 memories, more than the map draws at once/)).toBeInTheDocument();
    expect(renderer.model?.categories.map((c) => [c.category, c.count])).toEqual([
      ["preference.coding", 2],
      ["fact.project", 1],
    ]);
    expect(screen.queryByRole("group", { name: "Zoom level" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Lasso select" })).not.toBeInTheDocument();
  });

  it("opens one category's map from the list, and comes back", async () => {
    const { server, renderer, user } = await openAggregate();

    await user.click(screen.getByRole("button", { name: "Open Coding preference, 2 memories" }));

    expect(await screen.findByRole("heading", { name: /Coding preference · 2 memories/ })).toBeInTheDocument();
    await waitFor(() => expect(renderer.model?.scope).toBe("preference.coding"));
    expect(renderer.model?.memories.map((m) => m.id).sort()).toEqual(["a", "b"]);
    expect(server.calls.filter((c) => c.method === "map").map((c) => c.id ?? "*")).toEqual(["*", "preference.coding"]);

    await user.click(screen.getByRole("button", { name: /All categories/ }));
    await waitFor(() => expect(renderer.model?.aggregate).toBe(true));
  });

  it("opens a category clicked on the map", async () => {
    const { renderer } = await openAggregate();

    renderer.clickCategory("fact.project");

    await waitFor(() => expect(renderer.model?.memories.map((m) => m.id)).toEqual(["c"]));
  });
});

describe("the layout cache", () => {
  beforeEach(() => window.localStorage.clear());

  it("keeps the last layout per user, without memory text, and forgets it on sign-out", async () => {
    const { user } = await openMap();

    const raw = window.localStorage.getItem("recuerdos.map-layout.v1:alex:*");
    expect(raw).not.toBeNull();
    expect(Object.keys(JSON.parse(raw!).positions).sort()).toEqual(["a", "b", "c"]);
    expect(raw).not.toContain("pnpm");

    await user.click(screen.getByRole("button", { name: "Sign out" }));
    expect(window.localStorage.getItem("recuerdos.map-layout.v1:alex:*")).toBeNull();
  });
});

