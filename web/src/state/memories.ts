import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo } from "react";
import { ApiError, type ApiClient } from "../api/client";
import type {
  BulkEdit,
  BulkResult,
  Category,
  CategoryChange,
  CategoryUpdate,
  ConflictingMemory,
  IngestResult,
  MapData,
  Memory,
  MemoryStructure,
  Project,
  ProjectChange,
  RecallHit,
  SavedMemory,
  StructurePatch,
  Suggestion,
  Topic,
  GoalStatus,
} from "../api/types";
import { fieldsOf, patchOf, type FormValues } from "../domain/memoryForm";
import { describeError } from "./errors";

export function memoriesQueryKey(user: string) {
  return ["memories", user] as const;
}

export function categoriesQueryKey(user: string) {
  return ["categories", user] as const;
}

export function topicsQueryKey(user: string) {
  return ["topics", user] as const;
}

export function projectsQueryKey(user: string) {
  return ["projects", user] as const;
}

/** The topics a memory can be about, with counts. */
export function useTopics(client: ApiClient, user: string) {
  return useQuery<Topic[], Error>({ queryKey: topicsQueryKey(user), queryFn: () => client.listTopics() });
}

/** The projects memories apply to, with counts. */
export function useProjects(client: ApiClient, user: string) {
  return useQuery<Project[], Error>({ queryKey: projectsQueryKey(user), queryFn: () => client.listProjects() });
}

/** Every map of the user's; with `category`, the map of that category (`null` = the whole store). */
export function mapQueryKey(user: string, category?: string | null) {
  return category === undefined ? (["map", user] as const) : (["map", user, category ?? "*"] as const);
}

/** The map: memories, how they link, and the categories — of the whole store, or of one category. */
export function useMap(client: ApiClient, user: string, category: string | null = null) {
  return useQuery<MapData, Error>({
    queryKey: mapQueryKey(user, category),
    queryFn: () => client.getMap(category),
  });
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

/** What saving a new memory did, including the refusal the user can resolve. */
export type CreateOutcome =
  | { kind: "ok"; value: SavedMemory }
  /** It contradicts (or, without a model, nearly repeats) stored memories. */
  | { kind: "conflict"; message: string; conflicts: ConflictingMemory[] }
  | { kind: "failed"; message: string };

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
        queryClient.invalidateQueries({ queryKey: mapQueryKey(user) }),
        queryClient.invalidateQueries({ queryKey: topicsQueryKey(user) }),
        queryClient.invalidateQueries({ queryKey: projectsQueryKey(user) }),
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
      async save(
        base: Memory,
        values: FormValues,
        overwrite = false,
        structure: StructurePatch | null = null,
      ): Promise<SaveOutcome> {
        const fields = patchOf(base, values);
        if (fields === null && structure === null) return { kind: "unchanged" };
        const patch = { ...fields, ...structure };
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
      /**
       * Closes (or reopens) a goal. Only the user closes a goal, and this is
       * the user saying so (plan R12).
       */
      setGoalStatus: async (base: Memory, status: GoalStatus): Promise<SaveOutcome> => {
        try {
          const memory = await client.updateMemory(base.id, { goal_status: status, origin: "user_said" }, base.updated_at);
          await refresh();
          return { kind: "saved", memory };
        } catch (error) {
          if (error instanceof ApiError && error.status === 412) return { kind: "conflict" };
          return { kind: "failed", message: describeError(error) };
        }
      },
      createTopic: (id: string, label: string | null): Promise<Outcome<Topic>> => attempt(() => client.createTopic(id, label)),
      relabelTopic: (id: string, label: string | null): Promise<Outcome<Topic>> =>
        attempt(() => client.relabelTopic(id, label)),
      renameProject: (name: string, newName: string): Promise<Outcome<ProjectChange>> =>
        attempt(() => client.renameProject(name, newName)),
      mergeProject: (name: string, into: string): Promise<Outcome<ProjectChange>> =>
        attempt(() => client.mergeProject(name, into)),
      /** Read-only: proposals for the review queue. */
      suggest: async (ids: string[]): Promise<Outcome<Suggestion[]>> => {
        try {
          return { kind: "ok", value: await client.suggest(ids) };
        } catch (error) {
          return { kind: "failed", message: describeError(error) };
        }
      },
      create: async (values: FormValues, structure: MemoryStructure): Promise<CreateOutcome> => {
        try {
          const value = await client.createMemory(fieldsOf(values), structure);
          await refresh();
          return { kind: "ok", value };
        } catch (error) {
          if (error instanceof ApiError && error.code === "conflict" && error.conflicts.length > 0) {
            return { kind: "conflict", message: error.message, conflicts: error.conflicts };
          }
          return { kind: "failed", message: describeError(error) };
        }
      },
      ingest: (values: FormValues, useCategory: boolean): Promise<Outcome<IngestResult>> => {
        const fields = fieldsOf(values);
        return attempt(() =>
          client.ingest({ content: fields.content, category: useCategory ? fields.category : null, tags: fields.tags }),
        );
      },
      forget: (id: string) => attempt(() => client.forgetMemory(id)),
      /** Read-only; no refresh needed, but the same never-throw shape. */
      recall: async (query: string, limit = 10): Promise<Outcome<RecallHit[]>> => {
        try {
          return { kind: "ok", value: await client.recall(query, limit) };
        } catch (error) {
          return { kind: "failed", message: describeError(error) };
        }
      },
      restore: (id: string) => attempt(() => client.restoreMemory(id)),
      createCategory: (id: string, label: string | null): Promise<Outcome<Category>> =>
        attempt(() => client.createCategory(id, label)),
      updateCategory: (id: string, update: CategoryUpdate): Promise<Outcome<CategoryChange>> =>
        attempt(() => client.updateCategory(id, update)),
      mergeCategory: (id: string, into: string): Promise<Outcome<CategoryChange>> =>
        attempt(() => client.mergeCategory(id, into)),
      deleteCategory: (id: string, moveTo: string | null): Promise<Outcome<CategoryChange>> =>
        attempt(() => client.deleteCategory(id, moveTo)),
      bulkEdit: (ids: string[], edit: BulkEdit): Promise<Outcome<BulkResult>> => attempt(() => client.bulkEdit(ids, edit)),
    };
  }, [client, user, queryClient]);
}

export type MemoryActions = ReturnType<typeof useMemoryActions>;
