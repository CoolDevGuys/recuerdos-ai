import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import { FakeServer, memory } from "./api/fake";
import { memoryKeyStore } from "./state/keyStore";
import { FakeRenderer } from "./test/fakeRenderer";
import { ALEX_KEY, ALEX_MEMORIES, READER_KEY, renderApp, signedInAsAlex, standardServer } from "./test/renderApp";

beforeEach(() => window.history.pushState(null, "", "/ui/"));

function serverWithHomelab(): FakeServer {
  return new FakeServer().addAccount(ALEX_KEY, { user: "alex", scopes: ["read", "write"] }, [
    ...ALEX_MEMORIES,
    memory({ id: "h1", content: "Runs Proxmox at home", category: "homelab" }),
    memory({ id: "h2", content: "Has a Synology NAS", category: "homelab" }),
  ]);
}

async function openCategories(server: FakeServer = serverWithHomelab(), key = ALEX_KEY) {
  renderApp({ server, keyStore: memoryKeyStore(key) });
  const user = userEvent.setup();
  await user.click(await screen.findByRole("link", { name: "Categories" }));
  await screen.findByRole("table");
  return { server, user };
}

function row(name: RegExp | string) {
  return screen.getByRole("row", { name: typeof name === "string" ? new RegExp(name) : name });
}

describe("the categories manager", () => {
  it("is its own URL and lists every category with its count", async () => {
    await openCategories();

    expect(window.location.pathname).toBe("/ui/categories");
    expect(within(row("Coding preference")).getByText("2")).toBeInTheDocument();
    expect(within(row("Homelab")).getByText("homelab")).toBeInTheDocument();
    expect(within(row("Decision")).getByText("built-in")).toBeInTheDocument();
  });

  it("creates a category that can then be offered for a new memory", async () => {
    const { user } = await openCategories();

    await user.type(screen.getByLabelText("Id"), "Health.Sleep");
    await user.type(screen.getByLabelText(/Label/), "Sleep log");
    await user.click(screen.getByRole("button", { name: "Create" }));

    expect(await screen.findByRole("row", { name: /Sleep log/ })).toHaveTextContent("health.sleep");
    await user.click(screen.getByRole("link", { name: "List" }));
    await user.click(await screen.findByRole("button", { name: "+ Add memory" }));
    // A memory saved as written takes its category from what it is; the AI
    // is the one offered a category, as a hint.
    await user.click(screen.getByLabelText(/Let the AI classify it/));
    expect(screen.getByRole("option", { name: "Sleep log" })).toBeInTheDocument();
  });

  it("explains a bad id before sending it", async () => {
    const { server, user } = await openCategories();

    await user.type(screen.getByLabelText("Id"), "decision");
    await user.click(screen.getByRole("button", { name: "Create" }));

    expect(screen.getByText(/is a built-in category. Relabel it instead/)).toBeInTheDocument();
    expect(server.calls.some((c) => c.method === "createCategory")).toBe(false);
  });

  it("relabels a built-in, display only", async () => {
    const { server, user } = await openCategories();

    await user.click(screen.getByRole("button", { name: "Relabel Coding preference" }));
    const panel = screen.getByRole("dialog");
    await user.type(within(panel).getByLabelText("Label"), "How I code");
    await user.click(within(panel).getByRole("button", { name: "Save" }));

    expect(await screen.findByRole("row", { name: /How I code/ })).toHaveTextContent("default: Coding preference");
    expect(server.memoriesOf(ALEX_KEY).find((m) => m.id === "a")?.category).toBe("preference.coding");
  });

  it("renames a custom category, moving its memories", async () => {
    const { server, user } = await openCategories();

    await user.click(screen.getByRole("button", { name: "Edit Homelab" }));
    const panel = screen.getByRole("dialog");
    const id = within(panel).getByLabelText("Id");
    await user.clear(id);
    await user.type(id, "home.lab");
    expect(within(panel).getByText(/moves its 2 memories to home.lab/)).toBeInTheDocument();
    await user.click(within(panel).getByRole("button", { name: "Save" }));

    expect(await screen.findByText("Renamed to home.lab; 2 memories moved.")).toBeInTheDocument();
    expect(server.memoriesOf(ALEX_KEY).filter((m) => m.category === "home.lab")).toHaveLength(2);
  });

  it("merges a custom category into another", async () => {
    const { server, user } = await openCategories();

    await user.click(screen.getByRole("button", { name: "Merge Homelab" }));
    const panel = screen.getByRole("dialog");
    await user.selectOptions(within(panel).getByLabelText("Move its memories to"), "Project fact");
    await user.click(within(panel).getByRole("button", { name: "Merge into Project fact" }));

    await waitFor(() => expect(screen.queryByRole("row", { name: /Homelab/ })).not.toBeInTheDocument());
    expect(server.memoriesOf(ALEX_KEY).filter((m) => m.category === "fact.project")).toHaveLength(3);
  });

  it("will not delete a category with memories until told where they go", async () => {
    const { server, user } = await openCategories();

    await user.click(screen.getByRole("button", { name: "Delete Homelab" }));
    const panel = screen.getByRole("dialog");
    expect(within(panel).getByText(/still holds 2 memories/)).toBeInTheDocument();
    await user.click(within(panel).getByRole("button", { name: "Move and delete Homelab" }));
    expect(within(panel).getByRole("alert")).toHaveTextContent("Choose where its 2 memories should go.");
    expect(server.calls.some((c) => c.method === "deleteCategory" || c.method === "mergeCategory")).toBe(false);

    await user.selectOptions(within(panel).getByLabelText("Move its memories to"), "Skill");
    await user.click(within(panel).getByRole("button", { name: "Move and delete Homelab" }));

    expect(await screen.findByText("Homelab deleted; 2 memories moved to Skill.")).toBeInTheDocument();
    expect(server.memoriesOf(ALEX_KEY).filter((m) => m.category === "skill")).toHaveLength(2);
  });

  it("built-ins can only be relabelled", async () => {
    await openCategories();

    const decision = row("Decision");
    expect(within(decision).getByRole("button", { name: "Relabel Decision" })).toBeInTheDocument();
    expect(within(decision).queryByRole("button", { name: /Merge|Delete/ })).not.toBeInTheDocument();
  });

  it("offers no changes to a read-only key", async () => {
    await openCategories(standardServer(), READER_KEY);

    expect(screen.queryByRole("button", { name: "Create" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Relabel|Edit/ })).not.toBeInTheDocument();
  });
});

describe("bulk edits from the list", () => {
  it("re-files and tags the selected memories in one request", async () => {
    // What they are, and the tags: one bulk edit, under the save rules.
    const { server, user } = await signedInAsAlex();

    await user.click(screen.getByRole("button", { name: "Select" }));
    await user.click(screen.getByRole("checkbox", { name: /The user prefers pnpm/ }));
    await user.click(screen.getByRole("checkbox", { name: /Testing follows TDD/ }));
    const editor = screen.getByRole("region", { name: "2 memories selected" });
    await user.selectOptions(within(editor).getByLabelText("What they are"), "Rule");
    await user.type(within(editor).getByLabelText("Add tags"), "tooling");
    await user.click(within(editor).getByRole("button", { name: "Apply to 2 memories" }));

    expect(await screen.findByText("Updated 2 memories.")).toBeInTheDocument();
    const changed = server.memoriesOf(ALEX_KEY).filter((m) => m.id === "a" || m.id === "b");
    expect(changed.every((m) => m.kind === "rule" && m.tags.includes("tooling"))).toBe(true);
    expect(server.calls.filter((c) => c.method === "bulk")).toHaveLength(1);
    expect(screen.queryByRole("region", { name: /selected/ })).not.toBeInTheDocument();
  });

  it("selects everything shown, and asks for a change before sending", async () => {
    const { server, user } = await signedInAsAlex();

    await user.click(screen.getByRole("button", { name: "Select" }));
    await user.click(screen.getByRole("checkbox", { name: "Select all 3" }));
    const editor = screen.getByRole("region", { name: "3 memories selected" });
    await user.click(within(editor).getByRole("button", { name: "Apply to 3 memories" }));

    expect(within(editor).getByRole("alert")).toHaveTextContent("Choose something to change.");
    expect(server.calls.some((c) => c.method === "bulk")).toBe(false);
  });

  it("leaving select mode clears the selection", async () => {
    const { user } = await signedInAsAlex();

    await user.click(screen.getByRole("button", { name: "Select" }));
    await user.click(screen.getByRole("checkbox", { name: /The user prefers pnpm/ }));
    await user.click(screen.getByRole("button", { name: "Done selecting" }));

    expect(screen.queryByRole("region", { name: /selected/ })).not.toBeInTheDocument();
  });
});

describe("lasso selection on the map", () => {
  async function openMap(key = ALEX_KEY) {
    const renderer = new FakeRenderer();
    renderApp({ keyStore: memoryKeyStore(key), renderer });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("link", { name: "Map" }));
    await screen.findByRole("application", { name: /Map of 3 memories/ });
    await waitFor(() => expect(renderer.model).not.toBeNull());
    return { renderer, user };
  }

  it("turns the lasso on and edits what it encloses", async () => {
    const { renderer, user } = await openMap();

    await user.click(screen.getByRole("button", { name: "Lasso select" }));
    expect(renderer.lassoModes.at(-1)).toBe(true);
    renderer.lasso(["a", "c"]);

    const editor = await screen.findByRole("region", { name: "2 memories selected" });
    expect(renderer.lastView?.highlighted).toEqual(new Set(["a", "c"]));
    await user.selectOptions(within(editor).getByLabelText("About"), "Work");
    await user.click(within(editor).getByRole("button", { name: "Apply to 2 memories" }));

    expect(await screen.findByText("Updated 2 memories.")).toBeInTheDocument();
    await waitFor(() => expect(renderer.lastView?.highlighted ?? null).toBeNull());
  });

  it("says when the lasso caught nothing", async () => {
    const { renderer } = await openMap();

    renderer.lasso([]);

    expect(await screen.findByText(/The lasso caught no memories/)).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: /selected/ })).not.toBeInTheDocument();
  });

  it("is not offered to a read-only key", async () => {
    await openMap(READER_KEY);

    expect(screen.queryByRole("button", { name: "Lasso select" })).not.toBeInTheDocument();
  });
});
