import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { FakeServer } from "./api/fake";
import { memoryKeyStore } from "./state/keyStore";
import { ALEX_KEY, ALEX_MEMORIES, UNKNOWN_KEY, READER_KEY, renderApp, signIn, standardServer } from "./test/renderApp";

describe("signing in", () => {
  it("shows the user's memories after a valid key", async () => {
    const { keyStore } = renderApp();

    await signIn(ALEX_KEY);

    expect(await screen.findByText("The user prefers pnpm")).toBeInTheDocument();
    expect(screen.getByText("alex")).toBeInTheDocument();
    expect(screen.getByText(/Read & write key/)).toBeInTheDocument();
    expect(keyStore.read()).toBe(ALEX_KEY);
  });

  it("explains a refused key and keeps what was typed", async () => {
    const { keyStore } = renderApp();

    await signIn(UNKNOWN_KEY);

    expect(await screen.findByRole("alert")).toHaveTextContent("This key was not accepted");
    expect(screen.getByLabelText("API key")).toHaveValue(UNKNOWN_KEY);
    expect(keyStore.read()).toBeNull();
  });

  it("checks the key's shape on blur, before any request", async () => {
    renderApp();
    const user = userEvent.setup();

    await user.type(await screen.findByLabelText("API key"), "not-a-key");
    await user.tab();

    expect(screen.getByRole("alert")).toHaveTextContent("start with “ra_live_”");
    expect(screen.getByLabelText("API key")).toHaveAttribute("aria-invalid", "true");
  });

  it("restores the session from a key kept in this tab", async () => {
    renderApp({ keyStore: memoryKeyStore(ALEX_KEY) });

    expect(await screen.findByText("Testing follows TDD")).toBeInTheDocument();
  });

  it("asks again when the kept key has been revoked", async () => {
    const keyStore = memoryKeyStore(UNKNOWN_KEY);
    renderApp({ keyStore });

    expect(await screen.findByText(/no longer accepted/)).toBeInTheDocument();
    expect(keyStore.read()).toBeNull();
  });

  it("skips sign-in on a server that needs no key, and says so", async () => {
    renderApp({ server: standardServer().addAccount(null, { user: "default", scopes: ["admin"] }, ALEX_MEMORIES) });

    expect(await screen.findByText("The user prefers pnpm")).toBeInTheDocument();
    expect(screen.getByText(/runs without authentication/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Sign out" })).not.toBeInTheDocument();
  });

  it("signs out and forgets the key", async () => {
    const { keyStore } = renderApp();
    const user = await signIn(ALEX_KEY);
    await screen.findByText("The user prefers pnpm");

    await user.click(screen.getByRole("button", { name: "Sign out" }));

    expect(await screen.findByLabelText("API key")).toBeInTheDocument();
    expect(keyStore.read()).toBeNull();
  });
});

describe("scopes", () => {
  it("labels a read-only key", async () => {
    renderApp();

    await signIn(READER_KEY);

    expect(await screen.findByText(/Read-only key/)).toBeInTheDocument();
    expect(await screen.findByText("The user prefers pnpm")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "+ Add memory" })).not.toBeInTheDocument();
  });

  it("explains that a write-only key cannot show memories", async () => {
    const writer = "ra_live_0000000000000000000000000000000000000002";
    renderApp({ server: new FakeServer().addAccount(writer, { user: "agent", scopes: ["write"] }) });

    await signIn(writer);

    expect(await screen.findByRole("alert")).toHaveTextContent("cannot read memories");
  });
});

describe("the memory list", () => {
  it("filters by text", async () => {
    renderApp();
    const user = await signIn(ALEX_KEY);
    await screen.findByText("The user prefers pnpm");

    await user.type(screen.getByLabelText("Filter"), "tdd");

    const list = screen.getByRole("list");
    expect(within(list).getAllByRole("listitem")).toHaveLength(1);
    expect(within(list).getByText("Testing follows TDD")).toBeInTheDocument();
    expect(screen.getByText("1 of 3 memories")).toBeInTheDocument();
  });

  it("filters by topic, offering only topics present", async () => {
    renderApp();
    const user = await signIn(ALEX_KEY);
    await screen.findByText("The user prefers pnpm");

    const select = screen.getByLabelText("Topic");
    await waitFor(() =>
      expect(within(select).getAllByRole("option").map((o) => o.textContent)).toEqual([
        "All topics",
        "Coding (2)",
        "Work (1)",
      ]),
    );
    await user.selectOptions(select, "work");

    expect(within(screen.getByRole("list")).getAllByRole("listitem")).toHaveLength(1);
    expect(screen.getByText("Frontend uses Next.js")).toBeInTheDocument();
  });

  it("says when nothing matches", async () => {
    renderApp();
    const user = await signIn(ALEX_KEY);
    await screen.findByText("The user prefers pnpm");

    await user.type(screen.getByLabelText("Filter"), "kubernetes");

    expect(screen.getByText("No memory matches this filter.")).toBeInTheDocument();
  });

  it("says when there are no memories at all", async () => {
    renderApp({ server: new FakeServer().addAccount(ALEX_KEY, { user: "alex", scopes: ["read"] }) });

    await signIn(ALEX_KEY);

    expect(await screen.findByText("No memories stored yet.")).toBeInTheDocument();
  });
});
