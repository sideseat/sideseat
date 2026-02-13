import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { useFavoritesClient } from "@/lib/app-context";
import { favoritesKeys } from "./keys";
import type { FavoriteEntityType, SpanIdentifier } from "./types";

// ============================================================================
// QUERIES
// ============================================================================

/**
 * List all favorite IDs for a given entity type.
 * Used for "Favorites only" filter - fetches all user's favorites for filtering.
 */
export function useFavoriteIds(
  projectId: string,
  entityType: FavoriteEntityType,
  options?: { enabled?: boolean },
) {
  const favoritesClient = useFavoritesClient();
  return useQuery({
    queryKey: favoritesKeys.list(projectId, entityType),
    queryFn: () => favoritesClient.listFavorites(projectId, entityType),
    enabled: !!projectId && (options?.enabled ?? true),
    staleTime: 60_000, // Favorites don't change often
    placeholderData: (prev) => prev,
  });
}

/**
 * Check which IDs from a list are favorited.
 * Returns a Set of favorited IDs for efficient lookup.
 */
export function useCheckFavorites(
  projectId: string,
  entityType: FavoriteEntityType,
  ids: string[],
  options?: { enabled?: boolean },
) {
  const favoritesClient = useFavoritesClient();
  return useQuery({
    queryKey: favoritesKeys.check(projectId, entityType, ids),
    queryFn: () => favoritesClient.checkFavorites(projectId, entityType, ids),
    enabled: !!projectId && ids.length > 0 && (options?.enabled ?? true),
    staleTime: 60_000, // Favorites don't change often
    placeholderData: (prev) => prev, // Keep previous data while refetching
  });
}

/**
 * Check which spans are favorited.
 * Returns a Set of "trace_id:span_id" strings for efficient lookup.
 */
export function useCheckSpanFavorites(
  projectId: string,
  spans: SpanIdentifier[],
  options?: { enabled?: boolean },
) {
  const favoritesClient = useFavoritesClient();
  // Create a stable key from spans
  const ids = spans.map((s) => `${s.trace_id}:${s.span_id}`);
  return useQuery({
    queryKey: favoritesKeys.check(projectId, "span", ids),
    queryFn: () => favoritesClient.checkSpanFavorites(projectId, spans),
    enabled: !!projectId && spans.length > 0 && (options?.enabled ?? true),
    staleTime: 60_000,
    placeholderData: (prev) => prev,
  });
}

// ============================================================================
// MUTATIONS
// ============================================================================

interface ToggleFavoriteParams {
  projectId: string;
  entityType: FavoriteEntityType;
  entityId: string;
  secondaryId?: string;
  isFavorite: boolean; // Current state (true = remove, false = add)
}

/**
 * Toggle favorite status for an entity.
 * Uses optimistic updates with rollback on error.
 */
export function useToggleFavorite() {
  const queryClient = useQueryClient();
  const favoritesClient = useFavoritesClient();

  return useMutation({
    mutationFn: async ({
      projectId,
      entityType,
      entityId,
      secondaryId,
      isFavorite,
    }: ToggleFavoriteParams) => {
      if (isFavorite) {
        await favoritesClient.removeFavorite(projectId, entityType, entityId, secondaryId);
      } else {
        await favoritesClient.addFavorite(projectId, entityType, entityId, secondaryId);
      }
    },
    onMutate: async ({ projectId, entityType, entityId, secondaryId, isFavorite }) => {
      // Cancel any outgoing refetches for this project's favorites
      await queryClient.cancelQueries({ queryKey: favoritesKeys.all(projectId) });

      // Build the ID to toggle
      const toggleId = secondaryId ? `${entityId}:${secondaryId}` : entityId;

      // Snapshot current cache for all matching queries
      const previousQueries: Array<[readonly unknown[], Set<string> | undefined]> = [];

      // Get all check queries for this project and entity type
      const queries = queryClient.getQueriesData<Set<string>>({
        queryKey: [...favoritesKeys.all(projectId), "check", entityType],
      });

      queries.forEach(([queryKey, data]) => {
        previousQueries.push([queryKey, data]);

        if (data) {
          // Optimistically update the Set
          const newData = new Set(data);
          if (isFavorite) {
            newData.delete(toggleId);
          } else {
            newData.add(toggleId);
          }
          queryClient.setQueryData(queryKey, newData);
        }
      });

      return { previousQueries, projectId };
    },
    onError: (_err, vars, context) => {
      // Rollback all cached queries
      context?.previousQueries?.forEach(([queryKey, data]) => {
        queryClient.setQueryData(queryKey, data);
      });
      // Notify user of failure
      const action = vars.isFavorite ? "remove from" : "add to";
      toast.error(`Failed to ${action} favorites`);
    },
    onSettled: (_data, _err, { projectId }) => {
      // Invalidate all favorites for this project to ensure consistency
      queryClient.invalidateQueries({ queryKey: favoritesKeys.all(projectId) });
    },
  });
}
