import type { FavoriteEntityType } from "./types";

/**
 * Create a stable cache key from IDs by sorting them.
 * This ensures the same set of IDs always produces the same key.
 */
function createIdKey(ids: string[]): string {
  return [...ids].sort().join(",");
}

/**
 * Query key factories for favorites.
 * Separate namespace from other domains - NOT invalidated by SSE.
 */
export const favoritesKeys = {
  /** Base key for all favorites in a project */
  all: (projectId: string) => ["favorites", projectId] as const,

  /** Check favorites for a list of entity IDs */
  check: (projectId: string, entityType: FavoriteEntityType, ids: string[]) =>
    [...favoritesKeys.all(projectId), "check", entityType, createIdKey(ids)] as const,

  /** List all favorite IDs for an entity type (used for "favorites only" filter) */
  list: (projectId: string, entityType: FavoriteEntityType) =>
    [...favoritesKeys.all(projectId), "list", entityType] as const,
};
