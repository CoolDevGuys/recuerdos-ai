import { describe, expect, it } from "vitest";
import { describeIngest } from "./ingest";

describe("describeIngest", () => {
  it("counts what was stored", () => {
    expect(describeIngest({ outcome: "stored", memoryIds: ["a", "b"], understanding: true }).message).toBe("Saved 2 memories.");
  });

  it("says when there was no model to understand it", () => {
    expect(describeIngest({ outcome: "stored", memoryIds: ["a"], understanding: false }).message).toMatch(/stored exactly as written/);
  });

  it("explains each outcome that saved nothing", () => {
    expect(describeIngest({ outcome: "already_known", memoryIds: [], understanding: true }).tone).toBe("info");
    expect(describeIngest({ outcome: "nothing_durable", memoryIds: [], understanding: true }).message).toMatch(/nothing was saved/);
    expect(describeIngest({ outcome: "changed_without_storing", memoryIds: [], understanding: true }).message).toMatch(/updated or retired/);
  });
});
