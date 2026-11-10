import { describe, expect, it } from "vitest";
import { checkApiKey } from "./apiKey";

const VALID = "ra_live_1f4c8a209d4e5f6a7b8c9d0e1f2a3b4c5d6e7f80";

describe("checkApiKey", () => {
  it("accepts a well-formed key", () => {
    expect(checkApiKey(VALID)).toBeNull();
  });

  it("ignores surrounding whitespace from a paste", () => {
    expect(checkApiKey(`  ${VALID}\n`)).toBeNull();
  });

  it("reports an empty key", () => {
    expect(checkApiKey("   ")).toBe("empty");
  });

  it("reports a key without the scheme", () => {
    expect(checkApiKey(VALID.slice("ra_live_".length))).toBe("scheme");
  });

  it("reports a truncated key", () => {
    expect(checkApiKey(VALID.slice(0, -1))).toBe("shape");
  });

  it("reports characters a key never contains", () => {
    expect(checkApiKey(`${VALID.slice(0, -1)}z`)).toBe("shape");
  });
});
