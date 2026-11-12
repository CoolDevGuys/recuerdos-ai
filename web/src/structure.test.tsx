import { screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { memory } from "./api/fake";
import { ALEX_KEY, ALEX_MEMORIES, signedInAsAlex, standardServer } from "./test/renderApp";

// The web UI's structured-memory surfaces (ai-specs/memory-structure-plan.md
// S5): the Needs review queue, the Projects tab and the Topics tab.

beforeEach(() => window.history.pushState(null, "", "/ui/"));

const guessed = [
  memory({ id: "r1", content: "The NAS backups are in /mnt/tank", category: "fact.project", kind: "fact", topic: "coding", applies_to: { target: "general", project_name: null }, needs_review: true }),
  memory({ id: "r2", content: "The router is at 10.0.0.1", category: "fact.project", kind: "fact", topic: "coding", applies_to: { target: "general", project_name: null }, needs_review: true }),
];

function serverWith(extra = guessed, understanding = true) {
  const server = standardServer();
  server.understanding = understanding;
  server.addAccount(ALEX_KEY, { user: "alex", scopes: ["read", "write"] }, [...ALEX_MEMORIES, ...extra]);
  return server;
}

async function openTab(name: RegExp | string, server = serverWith()) {
  const { user } = await signedInAsAlex(server);
  await user.click(screen.getByRole("link", { name }));
  return { server, user };
}

describe("the Needs review queue", () => {
  it("counts what needs review in its tab, and lists only those", async () => {
    const { server } = await openTab(/Needs review/);

    expect(screen.getByRole("link", { name: "Needs review, 2 to review" })).toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: "Needs review · 2 memories" })).toBeInTheDocument();
    expect(screen.getByText("The NAS backups are in /mnt/tank")).toBeInTheDocument();
    expect(screen.queryByText("The user prefers pnpm")).not.toBeInTheDocument();
    expect(server.calls.some((c) => c.method === "suggest")).toBe(false);
  });

  it("files selected memories with the bulk editor, and they leave the queue", async () => {
    const { server, user } = await openTab(/Needs review/);

    await user.click(await screen.findByRole("checkbox", { name: "Select the 2 shown" }));
    const editor = screen.getByRole("region", { name: "2 memories selected" });
    await user.selectOptions(within(editor).getByLabelText("What they are"), "Reference");
    await user.selectOptions(within(editor).getByLabelText("About"), "Tech");
    await user.click(within(editor).getByRole("button", { name: "Apply to 2 memories" }));

    expect(await screen.findByText("Nothing to review: every memory says what it is and who it is for.")).toBeInTheDocument();
    const filed = server.memoriesOf(ALEX_KEY).filter((m) => m.id.startsWith("r"));
    expect(filed.every((m) => m.kind === "reference" && m.topic === "tech" && !m.needs_review)).toBe(true);
  });

  it("asks for suggestions, and accepts one or all", async () => {
    const server = serverWith();
    server.suggestions.set("r1", {
      kind: "reference",
      topic: "tech",
      applies_to: { target: "general", project_name: null },
      reason: "a path on the NAS",
    });
    server.suggestions.set("r2", {
      kind: "reference",
      topic: "tech",
      applies_to: { target: "general", project_name: null },
      reason: "an address",
    });
    const { user } = await openTab(/Needs review/, server);

    await user.click(await screen.findByRole("button", { name: "Suggest for the first 2" }));
    expect(await screen.findByText(/a path on the NAS/)).toBeInTheDocument();
    expect(server.memoriesOf(ALEX_KEY).find((m) => m.id === "r1")?.needs_review).toBe(true);

    await user.click(screen.getByRole("button", { name: "Accept all 2 suggestions" }));

    expect(await screen.findByText("Filed 2 memories.")).toBeInTheDocument();
    expect(server.calls.filter((c) => c.method === "bulk")).toHaveLength(1);
    expect(server.memoriesOf(ALEX_KEY).find((m) => m.id === "r2")).toMatchObject({ kind: "reference", needs_review: false });
  });

  it("does not offer Suggest without a model", async () => {
    await openTab(/Needs review/, serverWith(guessed, false));

    await screen.findByRole("heading", { name: /Needs review/ });
    expect(screen.queryByRole("button", { name: /Suggest/ })).not.toBeInTheDocument();
  });
});

describe("the Projects tab", () => {
  const projectMemories = [
    memory({ id: "p1", content: "Use cases are named by intent", kind: "convention", topic: "coding", applies_to: { target: "project", project_name: "fern" } }),
  ];

  it("lists projects with their memories and what else finds them", async () => {
    await openTab("Projects", serverWith(projectMemories));

    const quill = (await screen.findByRole("heading", { name: "quill" })).closest("li")!;
    expect(quill).toHaveTextContent("1 memory");
  });

  it("renames a project: its memories follow and the old name finds it", async () => {
    const { server, user } = await openTab("Projects", serverWith(projectMemories));
    const quill = (await screen.findByRole("heading", { name: "quill" })).closest("li")!;

    await user.click(within(quill).getByRole("button", { name: "Rename…" }));
    const name = within(quill).getByLabelText("New name for quill");
    await user.clear(name);
    await user.type(name, "quill-app");
    await user.click(within(quill).getByRole("button", { name: "Rename" }));

    expect(await screen.findByText("Renamed quill to quill-app.")).toBeInTheDocument();
    const renamed = (await screen.findByRole("heading", { name: "quill-app" })).closest("li")!;
    expect(renamed).toHaveTextContent("Also found by quill");
    expect(server.memoriesOf(ALEX_KEY).find((m) => m.id === "c")?.applies_to.project_name).toBe("quill-app");
  });

  it("refuses a name another project has, and says to merge", async () => {
    const { user } = await openTab("Projects", serverWith(projectMemories));
    const quill = (await screen.findByRole("heading", { name: "quill" })).closest("li")!;

    await user.click(within(quill).getByRole("button", { name: "Rename…" }));
    const name = within(quill).getByLabelText("New name for quill");
    await user.clear(name);
    await user.type(name, "fern");
    await user.click(within(quill).getByRole("button", { name: "Rename" }));

    expect(await within(quill).findByRole("alert")).toHaveTextContent(/merge/);
  });

  it("merges one project into another", async () => {
    const { server, user } = await openTab("Projects", serverWith(projectMemories));
    const fern = (await screen.findByRole("heading", { name: "fern" })).closest("li")!;

    await user.click(within(fern).getByRole("button", { name: "Merge into…" }));
    await user.selectOptions(within(fern).getByLabelText("Merge fern into"), "quill");
    await user.click(within(fern).getByRole("button", { name: "Merge" }));

    await waitFor(() => expect(screen.queryByRole("heading", { name: "fern" })).not.toBeInTheDocument());
    expect(server.memoriesOf(ALEX_KEY).find((m) => m.id === "p1")?.applies_to.project_name).toBe("quill");
  });

  it("opens a project's memories in the list", async () => {
    const { user } = await openTab("Projects", serverWith(projectMemories));
    const quill = (await screen.findByRole("heading", { name: "quill" })).closest("li")!;

    await user.click(within(quill).getByRole("button", { name: "Show its memories" }));

    expect(window.location.pathname).toBe("/ui/");
    expect(screen.getByLabelText("Applies to")).toHaveValue("project:quill");
    expect(screen.getByText("Frontend uses Next.js")).toBeInTheDocument();
    expect(screen.queryByText("The user prefers pnpm")).not.toBeInTheDocument();
  });
});

describe("the Topics tab", () => {
  it("lists the topics with counts, adds one and relabels one", async () => {
    const { server, user } = await openTab("Topics");

    const coding = await screen.findByText("Coding");
    expect(coding.closest("li")).toHaveTextContent("4 memories");

    await user.type(screen.getByLabelText("New topic"), "homelab");
    await user.type(screen.getByLabelText("Label (optional)"), "Home lab");
    await user.click(screen.getByRole("button", { name: "Add topic" }));
    expect(await screen.findByText("Added the topic Home lab.")).toBeInTheDocument();
    expect(await screen.findByText("Home lab")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Relabel Coding" }));
    const label = screen.getByLabelText("Label for “coding”");
    await user.clear(label);
    await user.type(label, "Software");
    await user.click(screen.getByRole("button", { name: "Save label" }));

    expect(await screen.findByText("Software")).toBeInTheDocument();
    expect(server.calls.some((c) => c.method === "relabelTopic" && c.id === "coding")).toBe(true);
  });

  it("explains a topic id it would refuse", async () => {
    const { server, user } = await openTab("Topics");

    await user.type(await screen.findByLabelText("New topic"), "Home Lab");
    await user.click(screen.getByRole("button", { name: "Add topic" }));

    expect(screen.getByRole("alert")).toHaveTextContent(/Start with a letter/);
    expect(server.calls.some((c) => c.method === "createTopic")).toBe(false);
  });
});
