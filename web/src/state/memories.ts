import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo } from "react";
import { ApiError, type ApiClient } from "../api/client";
import type { Category, IngestResult, Memory } from "../api/types";
import { fieldsOf, patchOf, type FormValues } from "../domain/memoryForm";
import { describeError } from "./errors";

export function memoriesQueryKey(user: string) {
  return ["memories", user] as const;
}

export function categoriesQueryKey(user: string) {
  return ["categories", user] as const;
}

/** Every active memory of the signed-in user. */
export function useMemories(client: ApiClient, user: string) {
  return useQuery<Memory[], Error>({
    queryKey: memoriesQueryKey(user),
    queryFn: () => client.listMemories(),
  });
}

/** The categories a memory can be filed under. */
export function useCategories(client: ApiClient, user: string) {
  return useQuery<Category[], Error>({
    queryKey: categoriesQueryKey(user),
    queryFn: () => client.listCategories(),
  });
}

export type SaveOutcome =
  | { kind: "saved"; memory: Memory }
  | { kind: "unchanged" }
  /** The memory changed elsewhere since the editor opened it. */
  | { kind: "conflict" }
  | { kind: "failed"; message: string };

export type Outcome<T> = { kind: "ok"; value: T } | { kind: "failed"; message: string };

/**
 * The write side, as plain async functions that never throw: each returns
 * what the UI needs to say next, and refreshes the cached lists after a
 * change. Components call these and render the outcome.
 */
export function useMemoryActions(client: ApiClient, user: string) {
  const queryClient = useQueryClient();

  return useMemo(() => {
    const refresh = () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: memoriesQueryKey(user) }),
        queryClient.invalidateQueries({ queryKey: categoriesQueryKey(user) }),
      ]);

    async function attempt<T>(run: () => Promise<T>): Promise<Outcome<T>> {
      try {
        const value = await run();
        await refresh();
        return { kind: "ok", value };
      } catch (error) {
        return { kind: "failed", message: describeError(error) };
      }
    }

    return {
      /**
       * Saves the changes `values` makes to `base`. Conditional on `base`
       * still being current unless `overwrite` is set.
       */
      async save(base: Memory, values: FormValues, overwrite = false): Promise<SaveOutcome> {
        const patch = patchOf(base, values);
        if (patch === null) return { kind: "unchanged" };
        try {
          const memory = await client.updateMemory(base.id, patch, overwrite ? null : base.updated_at);
          await refresh();
          return { kind: "saved", memory };
        } catch (error) {
          if (error instanceof ApiError && error.status === 412) return { kind: "conflict" };
          return { kind: "failed", message: describeError(error) };
        }
      },
      latest: (id: string) => attempt(() => client.getMemory(id)),
      create: (values: FormValues) => attempt(() => client.createMemory(fieldsOf(values))),
      ingest: (values: FormValues, useCategory: boolean): Promise<Outcome<IngestResult>> => {
        const fields = fieldsOf(values);
        return attempt(() =>
          client.ingest({ content: fields.content, category: useCategory ? fields.category : null, tags: fields.tags }),
        );
      },
      forget: (id: string) => attempt(() => client.forgetMemory(id)),
      restore: (id: string) => attempt(() => client.restoreMemory(id)),
    };
  }, [client, user, queryClient]);
}

export type MemoryActions = ReturnType<typeof useMemoryActions>;
