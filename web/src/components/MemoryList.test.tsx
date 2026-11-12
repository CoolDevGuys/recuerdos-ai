import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { memory } from "../api/fake";
import type { Memory } from "../api/types";
import { MemoryList, PAGE_SIZE } from "./MemoryList";

// 120 memories, newest first by `updated_at`: memory 0 is the most recent.
function memories(count: number): Memory[] {
  return Array.from({ length: count }, (_, i) => {
    const at = new Date(Date.UTC(2026, 0, 1) - i * 60_000).toISOString();
    return memory({
      id: `m${i}`,
      content: `Memory number ${i}`,
      category: i % 2 === 0 ? "preference.coding" : "decision",
      topic: i % 2 === 0 ? "coding" : "work",
      created_at: at,
      updated_at: at,
    });
  });
}

const rows = () => screen.getAllByRole("listitem");

describe("MemoryList paging", () => {
  it("renders one page of rows and pages through the rest", async () => {
    const user = userEvent.setup();
    render(<MemoryList memories={memories(120)} />);

    expect(rows()).toHaveLength(PAGE_SIZE);
    expect(screen.getByRole("status")).toHaveTextContent("120 memories · showing 1–50");
    const pages = screen.getByRole("navigation", { name: "Pages" });
    expect(pages).toHaveTextContent("Page 1 of 3");
    expect(screen.getByRole("button", { name: "← Previous" })).toBeDisabled();

    await user.click(screen.getByRole("button", { name: "Next →" }));
    await user.click(screen.getByRole("button", { name: "Next →" }));

    expect(rows()).toHaveLength(20);
    expect(screen.getByText("Memory number 100")).toBeInTheDocument();
    expect(screen.queryByText("Memory number 0")).not.toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("showing 101–120");
    expect(screen.getByRole("button", { name: "Next →" })).toBeDisabled();
  });

  it("shows no pager when everything fits on one page", () => {
    render(<MemoryList memories={memories(PAGE_SIZE)} />);

    expect(rows()).toHaveLength(PAGE_SIZE);
    expect(screen.queryByRole("navigation", { name: "Pages" })).not.toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(/^50 memories$/);
  });

  it("goes back to the first page when the filter or sort changes", async () => {
    const user = userEvent.setup();
    render(<MemoryList memories={memories(120)} />);
    await user.click(screen.getByRole("button", { name: "Next →" }));

    await user.selectOptions(screen.getByLabelText("Topic"), "work");

    expect(screen.getByRole("status")).toHaveTextContent("60 of 120 memories · showing 1–50");
    expect(screen.getByRole("navigation", { name: "Pages" })).toHaveTextContent("Page 1 of 2");

    await user.click(screen.getByRole("button", { name: "Next →" }));
    await user.selectOptions(screen.getByLabelText("Sort by"), "created");
    expect(screen.getByRole("navigation", { name: "Pages" })).toHaveTextContent("Page 1 of 2");
  });

  it("stays on a real page when the last page empties", async () => {
    const user = userEvent.setup();
    const all = memories(51);
    const { rerender } = render(<MemoryList memories={all} />);
    await user.click(screen.getByRole("button", { name: "Next →" }));
    expect(rows()).toHaveLength(1);

    rerender(<MemoryList memories={all.slice(0, 50)} />);

    expect(rows()).toHaveLength(50);
    expect(screen.queryByRole("navigation", { name: "Pages" })).not.toBeInTheDocument();
  });

  it("opens the page of a memory that was just changed", () => {
    const all = memories(120);
    const { rerender } = render(<MemoryList memories={all} />);

    rerender(<MemoryList memories={all} highlighted="m75" />);

    expect(screen.getByRole("navigation", { name: "Pages" })).toHaveTextContent("Page 2 of 3");
    expect(screen.getByText("Memory number 75")).toBeInTheDocument();
  });

  it("selects every memory the filter keeps, not only the page", async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn<(ids: readonly string[], on: boolean) => void>();
    render(<MemoryList memories={memories(120)} canWrite selected={new Set()} onSelect={onSelect} />);

    await user.click(screen.getByRole("button", { name: "Select" }));
    await user.selectOptions(screen.getByLabelText("Topic"), "work");
    await user.click(screen.getByRole("checkbox", { name: "Select all 60 matching" }));

    const [ids, on] = onSelect.mock.lastCall ?? [[], false];
    expect(on).toBe(true);
    expect(ids).toHaveLength(60);
  });
});
