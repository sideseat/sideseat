/**
 * Favorites API Types
 *
 * Universal favorites functionality for any entity type.
 */

/** Entity types that can be favorited */
export type FavoriteEntityType = "trace" | "session" | "span";

/** Identifier for span favorites (composite key) */
export interface SpanIdentifier {
  trace_id: string;
  span_id: string;
}

/** Request body for batch checking favorites */
export interface CheckFavoritesRequest {
  entity_type: FavoriteEntityType;
  ids?: string[];
  spans?: SpanIdentifier[];
}

/** Response from batch check favorites */
export interface CheckFavoritesResponse {
  favorites: string[];
}

/** Response from listing all favorites */
export interface ListFavoritesResponse {
  favorites: string[];
}

/** Response from add favorite operation */
export interface AddFavoriteResponse {
  created: boolean;
}

/** Response from remove favorite operation */
export interface RemoveFavoriteResponse {
  removed: boolean;
}
