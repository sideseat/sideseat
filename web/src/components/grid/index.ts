export type EntityType = "trace" | "session" | "span";

export interface GridContext {
  projectId: string;
  entityType: EntityType;
  trackDeletedIds?: (ids: string[]) => void;
  favoriteIds?: Set<string>;
  toggleFavorite?: (entityId: string, isFavorite: boolean, secondaryId?: string) => void;
  realtimeEnabled?: boolean;
}

/**
 * Extract entity ID based on entityType.
 * For traces: returns trace_id
 * For sessions: returns session_id
 * For spans: returns composite "trace_id:span_id"
 */
export function getEntityId(
  entityType: EntityType,
  data: { trace_id?: string; session_id?: string; span_id?: string } | undefined,
): string | undefined {
  if (!data) return undefined;
  if (entityType === "trace") return data.trace_id;
  if (entityType === "session") return data.session_id;
  // For spans, return composite ID
  return data.trace_id && data.span_id ? `${data.trace_id}:${data.span_id}` : undefined;
}

export { TokensCellRenderer } from "./tokens-cell";
export { CostCellRenderer } from "./cost-cell";
export { FavoriteCellRenderer } from "./favorite-cell";
export { ActionsCellRenderer } from "./actions-cell";
export { GridEmptyOverlay } from "./empty-overlay";
export { DeleteEntityDialog } from "./delete-entity-dialog";
