import { describe, expect, it } from "vitest";
import { accessFor } from "./access";

describe("accessFor", () => {
  it("lets a read key view but not edit", () => {
    expect(accessFor(["read"])).toEqual({ canRead: true, canWrite: false, label: "Read-only key" });
  });

  it("lets a read and write key do both", () => {
    expect(accessFor(["read", "write"])).toMatchObject({ canRead: true, canWrite: true });
  });

  it("does not let a write-only key read", () => {
    expect(accessFor(["write"])).toEqual({ canRead: false, canWrite: true, label: "Write-only key" });
  });

  it("treats admin as every scope", () => {
    expect(accessFor(["admin"])).toEqual({ canRead: true, canWrite: true, label: "Admin key" });
  });

  it("grants nothing without scopes", () => {
    expect(accessFor([])).toMatchObject({ canRead: false, canWrite: false });
  });
});
