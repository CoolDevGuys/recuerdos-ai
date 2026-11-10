import { describe, expect, it } from "vitest";
import { ApiError, HttpApiClient, NetworkError } from "./client";

function respond(status: number, body: unknown): typeof fetch {
  return async () => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

function recording(status: number, body: unknown) {
  const calls: { url: string; init: RequestInit | undefined }[] = [];
  const fetcher: typeof fetch = async (input, init) => {
    calls.push({ url: String(input), init });
    return respond(status, body)(input, init);
  };
  return { calls, fetcher };
}

describe("HttpApiClient", () => {
  it("sends the key as a bearer token, same-origin", async () => {
    const { calls, fetcher } = recording(200, { user: "alex", scopes: ["read"] });

    await new HttpApiClient("ra_live_key", fetcher).ping();

    expect(calls[0]?.url).toBe("/v1/ping");
    const headers = calls[0]?.init?.headers as Record<string, string>;
    expect(headers.Authorization).toBe("Bearer ra_live_key");
    expect(calls[0]?.init?.credentials).toBe("omit");
  });

  it("sends no credential when the server needs none", async () => {
    const { calls, fetcher } = recording(200, { user: "default", scopes: ["admin"] });

    await new HttpApiClient(null, fetcher).ping();

    const headers = calls[0]?.init?.headers as Record<string, string>;
    expect(headers.Authorization).toBeUndefined();
  });

  it("drops scopes it does not know", async () => {
    const client = new HttpApiClient("k", respond(200, { user: "alex", scopes: ["read", "future"] }));

    expect(await client.ping()).toEqual({ user: "alex", scopes: ["read"] });
  });

  it("surfaces the server's error code and message", async () => {
    const client = new HttpApiClient("k", respond(401, { error: { code: "unauthorized", message: "invalid API key" } }));

    await expect(client.ping()).rejects.toEqual(new ApiError(401, "unauthorized", "invalid API key"));
  });

  it("reports an unreachable server as a network error", async () => {
    const client = new HttpApiClient("k", async () => {
      throw new TypeError("Failed to fetch");
    });

    await expect(client.ping()).rejects.toBeInstanceOf(NetworkError);
  });

  it("parses the exported memories", async () => {
    const client = new HttpApiClient(
      "k",
      respond(200, {
        memories: [
          {
            id: "mem_1",
            content: "Testing follows TDD",
            category: "preference.coding",
            subcategory: "testing",
            tags: ["tdd"],
            confidence: 0.9,
            created_at: "2026-08-11T10:00:00Z",
            updated_at: "2026-08-11T10:00:00Z",
            superseded_by: null,
          },
        ],
      }),
    );

    const memories = await client.listMemories();

    expect(memories).toHaveLength(1);
    expect(memories[0]).toMatchObject({ id: "mem_1", subcategory: "testing", tags: ["tdd"] });
  });

  it("rejects a memory list of the wrong shape", async () => {
    const client = new HttpApiClient("k", respond(200, { memories: [{ id: 1 }] }));

    await expect(client.listMemories()).rejects.toMatchObject({ code: "malformed_response" });
  });
});
