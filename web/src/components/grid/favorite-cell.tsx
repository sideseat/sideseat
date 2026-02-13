import type { ICellRendererParams } from "ag-grid-community";
import { FavoriteButton } from "@/components/favorite-button";
import { getEntityId, type GridContext } from "./index";

type FavoriteCellParams = ICellRendererParams & {
  context?: GridContext;
};

/**
 * FavoriteCellRenderer reads favorite state from AG Grid context.
 * Parent component must call refreshCells() after mutation to trigger re-render.
 */
export function FavoriteCellRenderer(params: FavoriteCellParams) {
  const { entityType = "trace", projectId, favoriteIds, toggleFavorite } = params.context ?? {};
  const entityId = getEntityId(entityType, params.data);

  const disabled = !entityId || !projectId;
  const isFavorite = entityId ? (favoriteIds?.has(entityId) ?? false) : false;

  const handleToggle = () => {
    if (!entityId || !toggleFavorite) return;

    if (entityType === "span") {
      // For spans, split composite ID into trace_id and span_id
      const [traceId, spanId] = entityId.split(":");
      toggleFavorite(traceId, isFavorite, spanId);
    } else {
      toggleFavorite(entityId, isFavorite);
    }
  };

  return (
    <div
      className="flex items-center justify-center h-full"
      data-favorite-cell
      onClick={(e) => e.stopPropagation()}
    >
      <FavoriteButton
        isFavorite={isFavorite}
        disabled={disabled || !toggleFavorite}
        onToggle={handleToggle}
        className="h-6 w-6"
      />
    </div>
  );
}
