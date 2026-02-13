import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useOtelClient } from "@/lib/app-context";
import { otelKeys } from "../keys";
import type { PaginatedResponse, TraceSummary, SessionSummary, SpanSummary } from "../types";

interface DeleteTracesParams {
  projectId: string;
  traceIds: string[];
}

export function useDeleteTraces() {
  const queryClient = useQueryClient();
  const otelClient = useOtelClient();

  return useMutation({
    mutationFn: ({ projectId, traceIds }: DeleteTracesParams) =>
      otelClient.deleteTraces(projectId, traceIds),
    onMutate: async ({ projectId, traceIds }) => {
      // Cancel outgoing refetches
      await queryClient.cancelQueries({ queryKey: otelKeys.traces.lists(projectId) });

      // Snapshot previous value
      const previousData = queryClient.getQueriesData<PaginatedResponse<TraceSummary>>({
        queryKey: otelKeys.traces.lists(projectId),
      });

      // Optimistically remove traces from cache - use explicit setQueryData for each query
      // to ensure the update is applied correctly
      const traceIdSet = new Set(traceIds);
      previousData.forEach(([queryKey, data]) => {
        if (!data) return;
        const filteredData = data.data.filter((t) => !traceIdSet.has(t.trace_id));
        const newTotalItems = Math.max(
          0,
          data.meta.total_items - (data.data.length - filteredData.length),
        );
        queryClient.setQueryData(queryKey, {
          ...data,
          data: filteredData,
          meta: {
            ...data.meta,
            total_items: newTotalItems,
            total_pages: Math.max(1, Math.ceil(newTotalItems / data.meta.limit)),
          },
        });
      });

      return { previousData, projectId };
    },
    onError: (_err, _vars, context) => {
      // Rollback on error
      context?.previousData?.forEach(([queryKey, data]) => {
        queryClient.setQueryData(queryKey, data);
      });
      // Only invalidate on error to sync with server state
      if (context?.projectId) {
        queryClient.invalidateQueries({ queryKey: otelKeys.traces.lists(context.projectId) });
        queryClient.invalidateQueries({ queryKey: otelKeys.sessions.lists(context.projectId) });
      }
    },
    // Don't invalidate on success - optimistic update is sufficient
    // SSE or manual refresh will eventually sync if needed
  });
}

interface DeleteSessionsParams {
  projectId: string;
  sessionIds: string[];
}

export function useDeleteSessions() {
  const queryClient = useQueryClient();
  const otelClient = useOtelClient();

  return useMutation({
    mutationFn: ({ projectId, sessionIds }: DeleteSessionsParams) =>
      otelClient.deleteSessions(projectId, sessionIds),
    onMutate: async ({ projectId, sessionIds }) => {
      await queryClient.cancelQueries({ queryKey: otelKeys.sessions.lists(projectId) });

      const previousData = queryClient.getQueriesData<PaginatedResponse<SessionSummary>>({
        queryKey: otelKeys.sessions.lists(projectId),
      });

      // Optimistically remove sessions from cache - use explicit setQueryData for each query
      const sessionIdSet = new Set(sessionIds);
      previousData.forEach(([queryKey, data]) => {
        if (!data) return;
        const filteredData = data.data.filter((s) => !sessionIdSet.has(s.session_id));
        const newTotalItems = Math.max(
          0,
          data.meta.total_items - (data.data.length - filteredData.length),
        );
        queryClient.setQueryData(queryKey, {
          ...data,
          data: filteredData,
          meta: {
            ...data.meta,
            total_items: newTotalItems,
            total_pages: Math.max(1, Math.ceil(newTotalItems / data.meta.limit)),
          },
        });
      });

      return { previousData, projectId };
    },
    onError: (_err, _vars, context) => {
      context?.previousData?.forEach(([queryKey, data]) => {
        queryClient.setQueryData(queryKey, data);
      });
      // Only invalidate on error to sync with server state
      if (context?.projectId) {
        queryClient.invalidateQueries({ queryKey: otelKeys.sessions.lists(context.projectId) });
        queryClient.invalidateQueries({ queryKey: otelKeys.traces.lists(context.projectId) });
      }
    },
    // Don't invalidate on success - optimistic update is sufficient
    // SSE or manual refresh will eventually sync if needed
  });
}

interface DeleteSpansParams {
  projectId: string;
  /** Composite IDs in format "trace_id:span_id" */
  spanIds: string[];
}

export function useDeleteSpans() {
  const queryClient = useQueryClient();
  const otelClient = useOtelClient();

  return useMutation({
    mutationFn: ({ projectId, spanIds }: DeleteSpansParams) => {
      // Parse composite IDs into trace_id and span_id
      const spans = spanIds.map((id) => {
        const [trace_id, span_id] = id.split(":");
        return { trace_id, span_id };
      });
      return otelClient.deleteSpans(projectId, spans);
    },
    onMutate: async ({ projectId, spanIds }) => {
      await queryClient.cancelQueries({ queryKey: otelKeys.spans.lists(projectId) });

      const previousData = queryClient.getQueriesData<PaginatedResponse<SpanSummary>>({
        queryKey: otelKeys.spans.lists(projectId),
      });

      // Optimistically remove spans from cache
      const spanIdSet = new Set(spanIds);
      previousData.forEach(([queryKey, data]) => {
        if (!data) return;
        const filteredData = data.data.filter((s) => !spanIdSet.has(`${s.trace_id}:${s.span_id}`));
        const newTotalItems = Math.max(
          0,
          data.meta.total_items - (data.data.length - filteredData.length),
        );
        queryClient.setQueryData(queryKey, {
          ...data,
          data: filteredData,
          meta: {
            ...data.meta,
            total_items: newTotalItems,
            total_pages: Math.max(1, Math.ceil(newTotalItems / data.meta.limit)),
          },
        });
      });

      return { previousData, projectId };
    },
    onError: (_err, _vars, context) => {
      context?.previousData?.forEach(([queryKey, data]) => {
        queryClient.setQueryData(queryKey, data);
      });
      if (context?.projectId) {
        queryClient.invalidateQueries({ queryKey: otelKeys.spans.lists(context.projectId) });
      }
    },
  });
}
