import { useCallback, useEffect, useRef, useState } from "react";
import type { QueryClient } from "@tanstack/react-query";
import type { SseSpanEvent } from "@/api/otel/types";

const DELETED_IDS_RETENTION_MS = 30_000;
const SSE_DEBOUNCE_MS = 500;

/**
 * Track recently deleted entity IDs to prevent them from reappearing after SSE refetches.
 * IDs are automatically removed after the retention period.
 */
export function useRecentlyDeletedIds() {
  const [recentlyDeletedIds, setRecentlyDeletedIds] = useState<Set<string>>(new Set());
  const deleteTimeoutsRef = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map());

  const trackDeletedIds = useCallback((ids: string[]) => {
    setRecentlyDeletedIds((prev) => {
      const next = new Set(prev);
      ids.forEach((id) => next.add(id));
      return next;
    });

    // Schedule cleanup after retention period
    ids.forEach((id) => {
      const existing = deleteTimeoutsRef.current.get(id);
      if (existing) clearTimeout(existing);

      const timeout = setTimeout(() => {
        setRecentlyDeletedIds((prev) => {
          const next = new Set(prev);
          next.delete(id);
          return next;
        });
        deleteTimeoutsRef.current.delete(id);
      }, DELETED_IDS_RETENTION_MS);
      deleteTimeoutsRef.current.set(id, timeout);
    });
  }, []);

  useEffect(() => {
    const timeouts = deleteTimeoutsRef;
    return () => {
      timeouts.current.forEach((t) => clearTimeout(t));
      timeouts.current.clear();
    };
  }, []);

  return { recentlyDeletedIds, trackDeletedIds };
}

/**
 * Returns SSE handler that debounces refetch when event matches viewed entity.
 * For traces: matches event.trace_id
 * For sessions: matches event.session_id
 * For spans: matches composite "traceId:spanId" format
 */
export function useSseDetailRefresh(
  entityType: "trace" | "session" | "span",
  viewEntityId: string | null | undefined,
  queryClient: QueryClient,
) {
  const sseDebounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const handleSseEvent = useCallback(
    (event: SseSpanEvent) => {
      // Match based on entityType
      let eventEntityId: string;
      if (entityType === "span") {
        // Span entity ID is composite: "traceId:spanId"
        eventEntityId = `${event.trace_id}:${event.span_id}`;
      } else if (entityType === "trace") {
        eventEntityId = event.trace_id;
      } else {
        eventEntityId = event.session_id ?? "";
      }

      if (!viewEntityId || eventEntityId !== viewEntityId) return;

      if (sseDebounceRef.current) clearTimeout(sseDebounceRef.current);
      sseDebounceRef.current = setTimeout(() => {
        queryClient.refetchQueries({
          predicate: (q) => Array.isArray(q.queryKey) && q.queryKey.includes(viewEntityId),
          type: "active",
        });
        sseDebounceRef.current = null;
      }, SSE_DEBOUNCE_MS);
    },
    [entityType, viewEntityId, queryClient],
  );

  useEffect(() => {
    const ref = sseDebounceRef;
    return () => {
      if (ref.current) clearTimeout(ref.current);
    };
  }, []);

  return handleSseEvent;
}
