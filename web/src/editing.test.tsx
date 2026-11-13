import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ApiError } from "./api/client";
import { memory } from "./api/fake";
import { ALEX_KEY, ALEX_MEMORIES, signedInAsAlex, standardServer } from "./test/renderApp";

async function openMemory(content: string, server = standardServer()) {
  const { user } = await signedInAsAlex(server);
  await user.click(screen.getByRole("button", { name: new RegExp(content) }));
  const dialog = await screen.findByRole("dialog", { name: "Edit memory" });
  return { server, user, dialog };
}

describe("editing a memory", () => {
  it("saves only what changed, conditional on the version it opened", async () => {
    const { server, user, dialog } = await openMemory("Testing follows TDD");

    const content = within(dialog).getByLabelText("Memory");
    await user.clear(content);
    await user.type(content, "Testing follows TDD, with Vitest");
    await user.click(within(dialog).getByRole("button", { name: "Save changes" }));

    expect(await screen.findByText("Changes saved.")).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(await screen.findByText(/with Vitest/)).toBeInTheDocument();
    expect(server.calls.at(-1)).toEqual({
      method: "update",
      id: "b",
      version: "2026-08-12T00:00:00Z",
      patch: { content: "Testing follows TDD, with Vitest" },
    });
  });

  it("changes what a memory is and re-tags it", async () => {
    const { server, user, dialog } = await openMemory("The user prefers pnpm");

    await user.selectOptions(within(dialog).getByLabelText("What it is"), "Rule");
    await user.selectOptions(within(dialog).getByLabelText("About"), "Work");
    const tags = within(dialog).getByLabelText(/Tags/);
    await user.clear(tags);
    await user.type(tags, "#Tooling, pnpm, tooling");
    await user.click(within(dialog).getByRole("button", { name: "Save changes" }));

    await screen.findByText("Changes saved.");
    const saved = server.memoriesOf(ALEX_KEY).find((m) => m.id === "a");
    expect(saved).toMatchObject({ kind: "rule", topic: "work", tags: ["tooling", "pnpm"] });
    const sent = server.calls.find((c) => c.method === "update")?.patch;
    expect(sent).toMatchObject({ kind: "rule", topic: "work", origin: "user_said" });
    expect(sent).not.toHaveProperty("category");
  });

  it("moves a memory to a project", async () => {
    const { server, user, dialog } = await openMemory("Testing follows TDD");

    await user.selectOptions(within(dialog).getByLabelText("What it is"), "Convention");
    await user.click(within(dialog).getByRole("radio", { name: /One project/ }));
    await user.type(within(dialog).getByLabelText("Project"), "quill");
    await user.click(within(dialog).getByRole("button", { name: "Save changes" }));

    await screen.findByText("Changes saved.");
    expect(server.memoriesOf(ALEX_KEY).find((m) => m.id === "b")?.applies_to).toEqual({
      target: "project",
      project_name: "quill",
    });
  });

  it("closes a goal, and says what that means", async () => {
    const server = standardServer();
    server.addAccount(
      ALEX_KEY,
      { user: "alex", scopes: ["read", "write"] },
      [
        ...ALEX_MEMORIES,
        memory({ id: "g", content: "Run a half marathon in April", kind: "goal", topic: "health", goal_status: "active" }),
      ],
    );
    const { user, dialog } = await openMemory("Run a half marathon in April", server);

    expect(within(dialog).getByText(/Open: you are working towards it/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Mark achieved" }));

    await screen.findByText("Changes saved.");
    const closed = server.memoriesOf(ALEX_KEY).find((m) => m.id === "g");
    expect(closed?.goal_status).toBe("achieved");
    expect(server.calls.find((c) => c.method === "update")?.patch).toEqual({ goal_status: "achieved", origin: "user_said" });
    // A closed goal leaves the current list, and shows under Closed goals.
    expect(screen.queryByText("Run a half marathon in April")).not.toBeInTheDocument();
    await user.selectOptions(screen.getByLabelText("Show"), "closed");
    expect(await screen.findByText("Run a half marathon in April")).toBeInTheDocument();
  });

  it("confirms a guessed structure as it is", async () => {
    const server = standardServer();
    server.addAccount(
      ALEX_KEY,
      { user: "alex", scopes: ["read", "write"] },
      [...ALEX_MEMORIES, memory({ id: "r", content: "The NAS has 40 TB", kind: "fact", topic: "tech", needs_review: true })],
    );
    const { user, dialog } = await openMemory("The NAS has 40 TB", server);

    expect(within(dialog).getByText("Filed by a guess")).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "It is right as it is" }));

    await screen.findByText("Changes saved.");
    expect(server.memoriesOf(ALEX_KEY).find((m) => m.id === "r")?.needs_review).toBe(false);
  });

  it("explains an empty memory on blur and does not send it", async () => {
    const { server, user, dialog } = await openMemory("The user prefers pnpm");
    const content = within(dialog).getByLabelText("Memory");

    await user.clear(content);
    await user.tab();

    expect(within(dialog).getByText("Write what should be remembered.")).toBeInTheDocument();
    expect(content).toHaveAttribute("aria-invalid", "true");
    await user.click(within(dialog).getByRole("button", { name: "Save changes" }));
    expect(server.calls.filter((c) => c.method === "update")).toHaveLength(0);
  });

  it("closes without a request when nothing changed", async () => {
    const { server, user, dialog } = await openMemory("The user prefers pnpm");

    await user.click(within(dialog).getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(server.calls.filter((c) => c.method === "update")).toHaveLength(0);
  });

  it("keeps the typed text and explains a server refusal", async () => {
    const { server, user, dialog } = await openMemory("The user prefers pnpm");
    const content = within(dialog).getByLabelText("Memory");
    await user.type(content, " always");
    server.failNext(new ApiError(400, "validation_failed", "memory content is longer than 4000 characters"));

    await user.click(within(dialog).getByRole("button", { name: "Save changes" }));

    expect(await within(dialog).findByRole("alert")).toHaveTextContent("longer than 4000 characters");
    expect(content).toHaveValue("The user prefers pnpm always");
  });

  it("closes with Escape", async () => {
    const { user } = await openMemory("The user prefers pnpm");

    await user.keyboard("{Escape}");

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});

describe("an edit made elsewhere meanwhile", () => {
  it("is not overwritten silently", async () => {
    const { server, user, dialog } = await openMemory("The user prefers pnpm");
    server.editElsewhere(ALEX_KEY, "a", { content: "The user prefers bun" });

    const content = within(dialog).getByLabelText("Memory");
    await user.type(content, " strongly");
    await user.click(within(dialog).getByRole("button", { name: "Save changes" }));

    expect(await within(dialog).findByRole("alert")).toHaveTextContent("changed elsewhere");
    expect(content).toHaveValue("The user prefers pnpm strongly");
    expect(server.memoriesOf(ALEX_KEY).find((m) => m.id === "a")?.content).toBe("The user prefers bun");
  });

  it("can be loaded, discarding the local edit", async () => {
    const { server, user, dialog } = await openMemory("The user prefers pnpm");
    server.editElsewhere(ALEX_KEY, "a", { content: "The user prefers bun" });
    await user.type(within(dialog).getByLabelText("Memory"), " strongly");
    await user.click(within(dialog).getByRole("button", { name: "Save changes" }));
    await within(dialog).findByRole("alert");

    await user.click(within(dialog).getByRole("button", { name: "Load the latest" }));

    expect(within(dialog).getByLabelText("Memory")).toHaveValue("The user prefers bun");
    expect(within(dialog).queryByRole("alert")).not.toBeInTheDocument();
  });

  it("can be overwritten on purpose", async () => {
    const { server, user, dialog } = await openMemory("The user prefers pnpm");
    server.editElsewhere(ALEX_KEY, "a", { content: "The user prefers bun" });
    await user.type(within(dialog).getByLabelText("Memory"), " strongly");
    await user.click(within(dialog).getByRole("button", { name: "Save changes" }));
    await within(dialog).findByRole("alert");

    await user.click(within(dialog).getByRole("button", { name: "Save mine anyway" }));

    await screen.findByText("Changes saved.");
    expect(server.memoriesOf(ALEX_KEY).find((m) => m.id === "a")?.content).toBe("The user prefers pnpm strongly");
    expect(server.calls.at(-1)).toMatchObject({ method: "update", version: null });
  });
});

describe("forgetting a memory", () => {
  it("asks first, then forgets and offers Undo", async () => {
    const { server, user, dialog } = await openMemory("Frontend uses Next.js");

    await user.click(within(dialog).getByRole("button", { name: "Forget…" }));
    expect(server.calls.filter((c) => c.method === "forget")).toHaveLength(0);
    await user.click(within(dialog).getByRole("button", { name: "Forget" }));

    expect(await screen.findByText("Memory forgotten.")).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByText("Frontend uses Next.js")).not.toBeInTheDocument());

    await user.click(screen.getByRole("button", { name: "Undo" }));

    expect(await screen.findByText("Memory restored.")).toBeInTheDocument();
    expect(await screen.findByText("Frontend uses Next.js")).toBeInTheDocument();
    expect(server.calls.at(-1)).toEqual({ method: "restore", id: "c" });
  });

  it("can be called off", async () => {
    const { server, user, dialog } = await openMemory("Frontend uses Next.js");

    await user.click(within(dialog).getByRole("button", { name: "Forget…" }));
    await user.click(within(dialog).getByRole("button", { name: "Keep it" }));

    expect(within(dialog).getByRole("button", { name: "Forget…" })).toBeInTheDocument();
    expect(server.calls.filter((c) => c.method === "forget")).toHaveLength(0);
  });
});

describe("adding a memory", () => {
  async function openAdd(server = standardServer()) {
    const { user } = await signedInAsAlex(server);
    await user.click(screen.getByRole("button", { name: "+ Add memory" }));
    const dialog = await screen.findByRole("dialog", { name: "Add a memory" });
    return { server, user, dialog };
  }

  it("saves exactly what was written", async () => {
    const { server, user, dialog } = await openAdd();

    await user.type(within(dialog).getByLabelText("Memory"), "Deploys run on Fridays");
    await user.selectOptions(within(dialog).getByLabelText("What it is"), "Decision");
    await user.selectOptions(within(dialog).getByLabelText("About"), "Work");
    await user.click(within(dialog).getByRole("radio", { name: /One project/ }));
    await user.type(within(dialog).getByLabelText("Project"), "Quill");
    await user.type(within(dialog).getByLabelText(/Sub-label/), "release");
    await user.type(within(dialog).getByLabelText(/Tags/), "ops, deploys");
    await user.click(within(dialog).getByRole("button", { name: "Save memory" }));

    expect(await screen.findByText("Memory saved.")).toBeInTheDocument();
    expect(await screen.findByText("Deploys run on Fridays")).toBeInTheDocument();
    expect(server.memoriesOf(ALEX_KEY).at(-1)).toMatchObject({
      content: "Deploys run on Fridays",
      subcategory: "release",
      tags: ["ops", "deploys"],
    });
    expect(server.calls.find((call) => call.method === "create")?.structure).toEqual({
      kind: "decision",
      topic: "work",
      applies_to: { target: "project", project_name: "quill", new_project: true },
      origin: "user_said",
    });
  });

  it("asks what the memory is and who it is true for, and rules out what a kind cannot be", async () => {
    const { server, user, dialog } = await openAdd();

    await user.type(within(dialog).getByLabelText("Memory"), "Prefers pnpm");
    await user.click(within(dialog).getByRole("button", { name: "Save memory" }));

    expect(within(dialog).getByText("Choose what sort of memory this is.")).toBeInTheDocument();
    expect(within(dialog).getByText("Choose who or what it is true for.")).toBeInTheDocument();
    expect(server.calls).toHaveLength(0);

    await user.selectOptions(within(dialog).getByLabelText("What it is"), "Preference");
    expect(within(dialog).getByRole("radio", { name: /One project/ })).toBeDisabled();
    expect(within(dialog).getByRole("radio", { name: /General/ })).toBeDisabled();
    expect(within(dialog).getByRole("radio", { name: /Me, everywhere/ })).toBeEnabled();
  });

  it("shows why the daemon refused a save, and keeps what was typed", async () => {
    const server = standardServer();
    server.rejectNextSave = {
      code: "scope_contradiction",
      message: "This reads as being about one project (\"this repo\").",
    };
    const { user, dialog } = await openAdd(server);

    await user.type(within(dialog).getByLabelText("Memory"), "This repo uses pnpm");
    await user.selectOptions(within(dialog).getByLabelText("What it is"), "Fact");
    await user.selectOptions(within(dialog).getByLabelText("About"), "Coding");
    await user.click(within(dialog).getByRole("radio", { name: /General/ }));
    await user.click(within(dialog).getByRole("button", { name: "Save memory" }));

    expect(await within(dialog).findByRole("alert")).toHaveTextContent("about one project");
    expect(within(dialog).getByLabelText("Memory")).toHaveValue("This repo uses pnpm");
  });

  async function fillPreference(user: Awaited<ReturnType<typeof openAdd>>["user"], dialog: HTMLElement, text: string) {
    await user.type(within(dialog).getByLabelText("Memory"), text);
    await user.selectOptions(within(dialog).getByLabelText("What it is"), "Preference");
    await user.selectOptions(within(dialog).getByLabelText("About"), "Coding");
    await user.click(within(dialog).getByRole("radio", { name: /Me, everywhere/ }));
  }

  it("lets the user replace a memory a save contradicts", async () => {
    const server = standardServer();
    server.rejectNextSave = {
      code: "conflict",
      message: "It contradicts what is stored for me.",
      conflicts: [{ id: "a", content: "The user prefers pnpm" }],
    };
    const { user, dialog } = await openAdd(server);

    await fillPreference(user, dialog, "The user now prefers npm");
    await user.click(within(dialog).getByRole("button", { name: "Save memory" }));

    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent("This may contradict a memory you already have");
    expect(alert).toHaveTextContent("The user prefers pnpm");
    await user.click(within(alert).getByRole("button", { name: "Replace it" }));

    expect(await screen.findByText("Memory saved.")).toBeInTheDocument();
    expect(server.calls.at(-1)).toMatchObject({ method: "create", structure: { replaces: "a" } });
    expect(server.memoriesOf(ALEX_KEY).some((m) => m.id === "a")).toBe(false);
  });

  it("lets the user keep both when each holds", async () => {
    const server = standardServer();
    server.rejectNextSave = {
      code: "conflict",
      message: "It is very close to what is stored for me.",
      conflicts: [{ id: "a", content: "The user prefers pnpm" }],
    };
    const { user, dialog } = await openAdd(server);

    await fillPreference(user, dialog, "The user prefers pnpm for monorepos");
    await user.click(within(dialog).getByRole("button", { name: "Save memory" }));
    await user.click(within(await within(dialog).findByRole("alert")).getByRole("button", { name: "Keep both" }));

    expect(await screen.findByText("Memory saved.")).toBeInTheDocument();
    expect(server.calls.at(-1)).toMatchObject({ method: "create", structure: { coexists_with: ["a"] } });
  });

  it("says so when the memory is already stored", async () => {
    const { server, user, dialog } = await openAdd();

    await fillPreference(user, dialog, "The user prefers pnpm");
    await user.click(within(dialog).getByRole("button", { name: "Save memory" }));

    expect(await screen.findByText("You already have this memory, so nothing new was saved.")).toBeInTheDocument();
    expect(server.memoriesOf(ALEX_KEY).filter((m) => m.content === "The user prefers pnpm")).toHaveLength(1);
  });

  it("requires the text", async () => {
    const { server, user, dialog } = await openAdd();

    await user.click(within(dialog).getByRole("button", { name: "Save memory" }));

    expect(within(dialog).getByText("Write what should be remembered.")).toBeInTheDocument();
    expect(server.calls).toHaveLength(0);
  });

  it("can hand the text to the AI, which may store it", async () => {
    const { server, user, dialog } = await openAdd();

    await user.type(within(dialog).getByLabelText("Memory"), "We moved CI to GitHub Actions");
    await user.click(within(dialog).getByLabelText(/Let the AI classify it/));
    expect(within(dialog).queryByLabelText(/Sub-label/)).not.toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Save memory" }));

    expect(await screen.findByText("Saved 1 memory.")).toBeInTheDocument();
    expect(server.calls.at(-2)).toEqual({ method: "ingest" });
  });

  it("says when the AI found it already known", async () => {
    const server = standardServer();
    server.ingestOutcome = "already_known";
    const { user, dialog } = await openAdd(server);

    await user.type(within(dialog).getByLabelText("Memory"), "The user prefers pnpm");
    await user.click(within(dialog).getByLabelText(/Let the AI classify it/));
    await user.click(within(dialog).getByRole("button", { name: "Save memory" }));

    expect(await screen.findByText(/Already known/)).toBeInTheDocument();
  });
});

describe("with a read-only key", () => {
  it("shows a memory without any way to change it", async () => {
    const { renderApp, READER_KEY } = await import("./test/renderApp");
    const { memoryKeyStore } = await import("./state/keyStore");
    renderApp({ keyStore: memoryKeyStore(READER_KEY) });
    await screen.findByText("The user prefers pnpm");

    const user = (await import("@testing-library/user-event")).default.setup();
    await user.click(screen.getByRole("button", { name: /The user prefers pnpm/ }));

    const dialog = await screen.findByRole("dialog", { name: "Memory" });
    expect(within(dialog).getByLabelText("Memory")).toHaveAttribute("readonly");
    expect(within(dialog).queryByRole("button", { name: "Save changes" })).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Forget…" })).not.toBeInTheDocument();
  });
});
