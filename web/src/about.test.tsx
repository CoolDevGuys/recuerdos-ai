import { screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { memoryKeyStore } from "./state/keyStore";
import { ALEX_KEY, ALEX_MEMORIES, renderApp, signedInAsAlex, standardServer } from "./test/renderApp";
import { memory } from "./api/fake";
import userEvent from "@testing-library/user-event";

// The About page: the daemon's version for every key, its configuration for
// an admin.

beforeEach(() => window.history.pushState(null, "", "/ui/"));

const ADMIN_KEY = "ra_live_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

describe("the About page", () => {
  it("shows the version and the key, and points a non-admin at `recuerdos-ai config`", async () => {
    const { user } = await signedInAsAlex();

    await user.click(screen.getByRole("link", { name: "About" }));

    expect(await screen.findByRole("heading", { name: "Recuerdos AI 0.6.1" })).toBeInTheDocument();
    expect(window.location.pathname).toBe("/ui/about");
    const key = screen.getByRole("heading", { name: "This key" }).parentElement!;
    expect(key).toHaveTextContent("alex");
    expect(key).toHaveTextContent("read, write");
    expect(screen.getByText(/shown to an admin key/)).toBeInTheDocument();
    expect(screen.queryByText("/etc/recuerdos-ai/recuerdos-ai.toml")).not.toBeInTheDocument();
  });

  it("shows an admin the config file, the overrides by name and the settings in effect", async () => {
    const server = standardServer().addAccount(ADMIN_KEY, { user: "ops", scopes: ["admin"] }, ALEX_MEMORIES);
    window.history.pushState(null, "", "/ui/about");
    renderApp({ server, keyStore: memoryKeyStore(ADMIN_KEY) });

    expect(await screen.findByText("/etc/recuerdos-ai/recuerdos-ai.toml")).toBeInTheDocument();
    expect(screen.getByText("From --config")).toBeInTheDocument();
    expect(screen.getByText("recuerdos-ai serve --config /etc/recuerdos-ai/recuerdos-ai.toml")).toBeInTheDocument();
    const overrides = screen.getByRole("list", { name: "Overrides" });
    expect(within(overrides).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      "RECUERDOS_AI_AUTH__MODE",
      "RECUERDOS_AI_SERVER__PORT",
    ]);
    const understanding = screen.getByRole("region", { name: "understanding" });
    expect(understanding).toHaveTextContent("OPENAI_API_KEY (set)");
  });

  it("says when the named config file does not exist", async () => {
    const server = standardServer().addAccount(ADMIN_KEY, { user: "ops", scopes: ["admin"] }, ALEX_MEMORIES);
    server.daemon = { ...server.daemon, config_file: { path: "/nope.toml", from: "env", exists: false } };
    renderApp({ server, keyStore: memoryKeyStore(ADMIN_KEY) });
    await userEvent.setup().click(await screen.findByRole("link", { name: "About" }));

    expect(await screen.findByText(/From RECUERDOS_AI_CONFIG · the file does not exist/)).toBeInTheDocument();
  });

  it("opens straight from its address", async () => {
    window.history.pushState(null, "", "/ui/about");
    renderApp({ keyStore: memoryKeyStore(ALEX_KEY) });

    expect(await screen.findByRole("heading", { name: "Recuerdos AI 0.6.1" })).toBeInTheDocument();
  });
});

describe("who you are", () => {
  const legacy = [
    memory({
      id: "p1",
      content: "Alejandro Romero is a Senior Backend Software Engineer in Berlin",
      category: "fact.person",
      kind: "fact",
      topic: "people",
      applies_to: { target: "general", project_name: null },
    }),
    memory({
      id: "p2",
      content: "The user's sister Ana is a nurse",
      category: "fact.person",
      kind: "fact",
      topic: "family",
      applies_to: { target: "general", project_name: null },
    }),
  ];

  it("saves your names, and sends the memories about you filed as someone else's to review", async () => {
    const server = standardServer();
    server.addAccount(ALEX_KEY, { user: "alex", scopes: ["read", "write"] }, [...ALEX_MEMORIES, ...legacy]);
    const { user } = await signedInAsAlex(server);
    await user.click(screen.getByRole("link", { name: "About" }));

    await user.type(await screen.findByLabelText("Your name"), "Alejandro Romero");
    await user.type(screen.getByLabelText("Other names you go by"), "Alejandro, Alex");
    await user.click(screen.getByRole("button", { name: "Save names" }));

    expect(await screen.findByText(/1 memory about you was filed as someone else's/)).toBeInTheDocument();
    expect(server.memoriesOf(ALEX_KEY).find((m) => m.id === "p1")?.needs_review).toBe(true);
    expect(server.memoriesOf(ALEX_KEY).find((m) => m.id === "p2")?.needs_review).toBe(false);
    expect(screen.getByRole("link", { name: "Needs review, 1 to review" })).toBeInTheDocument();

    // From the queue, one click files it as yours.
    await user.click(screen.getByRole("link", { name: /Needs review/ }));
    await user.click(await screen.findByRole("button", { name: "It’s about me" }));
    expect(await screen.findByText("Filed as your personal information.")).toBeInTheDocument();
    expect(server.memoriesOf(ALEX_KEY).find((m) => m.id === "p1")).toMatchObject({
      applies_to: { target: "me" },
      needs_review: false,
    });
  });

  it("shows a read-only key your names without a way to change them", async () => {
    window.history.pushState(null, "", "/ui/about");
    renderApp({ keyStore: memoryKeyStore("ra_live_0000000000000000000000000000000000000001") });

    expect(await screen.findByLabelText("Your name")).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Save names" })).not.toBeInTheDocument();
  });
});

describe("personal information", () => {
  it("is its own group in the list: facts about you, apart from other facts", async () => {
    const server = standardServer();
    server.addAccount(ALEX_KEY, { user: "alex", scopes: ["read", "write"] }, [
      ...ALEX_MEMORIES,
      memory({ id: "m1", content: "The user is an expert in PHP", kind: "fact", topic: "work", applies_to: { target: "me", project_name: null } }),
    ]);
    const { user } = await signedInAsAlex(server);

    await user.selectOptions(screen.getByLabelText("Kind"), "Personal information (1)");

    expect(screen.getByText("The user is an expert in PHP")).toBeInTheDocument();
    expect(screen.queryByText("Frontend uses Next.js")).not.toBeInTheDocument();
    expect(screen.getByText(/Personal information · Work · you/)).toBeInTheDocument();
  });
});

