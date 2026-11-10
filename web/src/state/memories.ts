import { useQuery } from "@tanstack/react-query";
import type { ApiClient } from "../api/client";
import type { Memory } from "../api/types";

export function memoriesQueryKey(user: string) {
  return ["memories", user] as const;
}

/** Every active memory of the signed-in user. */
export function useMemories(client: ApiClient, user: string) {
  return useQuery<Memory[], Error>({
    queryKey: memoriesQueryKey(user),
    queryFn: () => client.listMemories(),
  });
}
