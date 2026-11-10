import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ApiError } from "./api/client";
import { ALEX_KEY, signedInAsAlex, standardServer } from "./test/renderApp";

async function openMemory(content: string) {
  const { server, user } = await signedInAsAlex();
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

  it("re-categorises and re-tags a memory", async () => {
    const { server, user, dialog } = await openMemory("The user prefers pnpm");

    await user.selectOptions(within(dialog).getByLabelText("Category"), "Project fact");
    const tags = within(dialog).getByLabelText(/Tags/);
    await user.clear(tags);
    await user.type(tags, "#Tooling, pnpm, tooling");
    await user.click(within(dialog).getByRole("button", { name: "Save changes" }));

    await screen.findByText("Changes saved.");
    const saved = server.memoriesOf(ALEX_KEY).find((m) => m.id === "a");
    expect(saved).toMatchObject({ category: "fact.project", tags: ["tooling", "pnpm"] });
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
    await user.selectOptions(within(dialog).getByLabelText("Category"), "Decision");
    await user.type(within(dialog).getByLabelText(/Sub-label/), "release");
    await user.type(within(dialog).getByLabelText(/Tags/), "ops, deploys");
    await user.click(within(dialog).getByRole("button", { name: "Save memory" }));

    expect(await screen.findByText("Memory saved.")).toBeInTheDocument();
    expect(await screen.findByText("Deploys run on Fridays")).toBeInTheDocument();
    expect(server.memoriesOf(ALEX_KEY).at(-1)).toMatchObject({
      content: "Deploys run on Fridays",
      category: "decision",
      subcategory: "release",
      tags: ["ops", "deploys"],
    });
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
