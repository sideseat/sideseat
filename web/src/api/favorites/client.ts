import type { ApiClient } from "../api-client";
import type {
  AddFavoriteResponse,
  CheckFavoritesRequest,
  CheckFavoritesResponse,
  FavoriteEntityType,
  ListFavoritesResponse,
  RemoveFavoriteResponse,
  SpanIdentifier,
} from "./types";

/**
 * Client for favorites API.
 * Universal favorites functionality that works with any entity type.
 */
export class FavoritesClient {
  private client: ApiClient;

  constructor(client: ApiClient) {
    this.client = client;
  }

  /** Build favorites base path for a project */
  private basePath(projectId: string): string {
    return `/project/${projectId}/favorites`;
  }

  /** Add a favorite (trace or session) */
  async addFavorite(
    projectId: string,
    entityType: FavoriteEntityType,
    entityId: string,
    secondaryId?: string,
  ): Promise<AddFavoriteResponse> {
    const path = secondaryId
      ? `${this.basePath(projectId)}/${entityType}/${entityId}/${secondaryId}`
      : `${this.basePath(projectId)}/${entityType}/${entityId}`;
    return this.client.put<AddFavoriteResponse>(path);
  }

  /** Remove a favorite (trace or session) */
  async removeFavorite(
    projectId: string,
    entityType: FavoriteEntityType,
    entityId: string,
    secondaryId?: string,
  ): Promise<RemoveFavoriteResponse> {
    const path = secondaryId
      ? `${this.basePath(projectId)}/${entityType}/${entityId}/${secondaryId}`
      : `${this.basePath(projectId)}/${entityType}/${entityId}`;
    return this.client.delete<RemoveFavoriteResponse>(path);
  }

  /** Batch check if entities are favorited */
  async checkFavorites(
    projectId: string,
    entityType: FavoriteEntityType,
    ids: string[],
  ): Promise<Set<string>> {
    if (ids.length === 0) {
      return new Set();
    }
    const body: CheckFavoritesRequest = { entity_type: entityType, ids };
    const response = await this.client.post<CheckFavoritesResponse>(
      `${this.basePath(projectId)}/check`,
      body,
    );
    return new Set(response.favorites);
  }

  /** Batch check if spans are favorited */
  async checkSpanFavorites(projectId: string, spans: SpanIdentifier[]): Promise<Set<string>> {
    if (spans.length === 0) {
      return new Set();
    }
    const body: CheckFavoritesRequest = { entity_type: "span", spans };
    const response = await this.client.post<CheckFavoritesResponse>(
      `${this.basePath(projectId)}/check`,
      body,
    );
    return new Set(response.favorites);
  }

  /** List all favorite IDs for an entity type (for "favorites only" filter) */
  async listFavorites(projectId: string, entityType: FavoriteEntityType): Promise<string[]> {
    const response = await this.client.get<ListFavoritesResponse>(
      `${this.basePath(projectId)}/list/${entityType}`,
    );
    return response.favorites;
  }
}
