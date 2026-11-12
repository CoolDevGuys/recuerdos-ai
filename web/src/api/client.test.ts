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

    expect(await client.ping()).toEqual({ user: "alex", scopes: ["read"], understanding: false });
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

  it("sends an edit with the version it is based on", async () => {
    const { calls, fetcher } = recording(200, {
      id: "mem_1",
      content: "x",
      category: "decision",
      tags: [],
      confidence: 1,
      created_at: "2026-08-11T10:00:00Z",
      updated_at: "2026-08-12T10:00:00Z",
    });

    await new HttpApiClient("k", fetcher).updateMemory("mem_1", { content: "x" }, "2026-08-11T10:00:00Z");

    expect(calls[0]?.url).toBe("/v1/memories/mem_1");
    expect(calls[0]?.init?.method).toBe("PATCH");
    expect((calls[0]!.init!.headers as Record<string, string>)["If-Match"]).toBe('"2026-08-11T10:00:00Z"');
    expect(calls[0]?.init?.body).toBe('{"content":"x"}');
  });

  it("sends an unconditional edit without If-Match", async () => {
    const { calls, fetcher } = recording(200, {
      id: "mem_1",
      content: "x",
      category: "decision",
      created_at: "2026-08-11T10:00:00Z",
      updated_at: "2026-08-12T10:00:00Z",
    });

    await new HttpApiClient("k", fetcher).updateMemory("mem_1", { content: "x" }, null);

    expect((calls[0]!.init!.headers as Record<string, string>)["If-Match"]).toBeUndefined();
  });

  it("reports a stale edit as a 412", async () => {
    const client = new HttpApiClient(
      "k",
      respond(412, { error: { code: "precondition_failed", message: "memory changed since it was read" } }),
    );

    await expect(client.updateMemory("m", { content: "x" }, "v")).rejects.toMatchObject({ status: 412 });
  });

  it("forgets with DELETE and accepts the empty 204", async () => {
    const calls: string[] = [];
    const client = new HttpApiClient("k", async (input, init) => {
      calls.push(`${init?.method} ${String(input)}`);
      return new Response(null, { status: 204 });
    });

    await client.forgetMemory("mem 1");

    expect(calls).toEqual(["DELETE /v1/memories/mem%201"]);
  });

  it("asks the pipeline to wait and parses its outcome", async () => {
    const { calls, fetcher } = recording(200, {
      job_id: "j",
      status: "succeeded",
      memory_ids: ["m1"],
      outcome: "stored",
      understanding: true,
    });

    const result = await new HttpApiClient("k", fetcher).ingest({ content: "We use Rust", category: null, tags: [] });

    expect(JSON.parse(String(calls[0]?.init?.body))).toEqual({ content: "We use Rust", tags: [], client: "web-ui", wait: true });
    expect(result).toEqual({ outcome: "stored", memoryIds: ["m1"], understanding: true });
  });

  it("parses the category list", async () => {
    const client = new HttpApiClient(
      "k",
      respond(200, { categories: [{ id: "decision", label: "Decision", built_in: true, count: 2 }] }),
    );

    expect(await client.listCategories()).toEqual([
      { id: "decision", label: "Decision", default_label: "Decision", built_in: true, known: true, count: 2 },
    ]);
  });

  it("sends a bulk edit in the daemon's shape", async () => {
    const { calls, fetcher } = recording(200, { memories: [], changed: 0, reembedded: 0 });

    await new HttpApiClient("k", fetcher).bulkEdit(["a", "b"], { category: "skill", subcategory: null, addTags: ["rust"] });

    expect(calls[0]?.url).toBe("/v1/memories:bulk");
    expect(calls[0]?.init?.method).toBe("POST");
    expect(JSON.parse(String(calls[0]?.init?.body))).toEqual({
      ids: ["a", "b"],
      set: { category: "skill", subcategory: null },
      add_tags: ["rust"],
      remove_tags: [],
    });
  });

  it("addresses category changes by id, with the destination of a delete in the query", async () => {
    const change = { category: null, moved: 3, reembedded: 3 };
    const { calls, fetcher } = recording(200, change);
    const client = new HttpApiClient("k", fetcher);

    expect(await client.deleteCategory("home lab", "fact.project")).toEqual(change);
    await client.updateCategory("homelab", { label: null, id: "home" });
    await client.mergeCategory("homelab", "skill");

    expect(calls.map((c) => [c.init?.method, c.url])).toEqual([
      ["DELETE", "/v1/categories/home%20lab?move_to=fact.project"],
      ["PATCH", "/v1/categories/homelab"],
      ["POST", "/v1/categories/homelab/merge"],
    ]);
    expect(JSON.parse(String(calls[1]?.init?.body))).toEqual({ label: null, id: "home" });
  });
});
