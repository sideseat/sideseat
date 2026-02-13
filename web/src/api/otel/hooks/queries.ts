import { useQuery, keepPreviousData } from "@tanstack/react-query";
import { useOtelClient } from "@/lib/app-context";
import { otelKeys, extractListParams, extractTraceListParams } from "../keys";
import type {
  FilterOptionsParams,
  ListTracesParams,
  ListSpansParams,
  ListSessionsParams,
  MessagesParams,
  ProjectStatsParams,
  TraceDetailParams,
  SpanDetailParams,
  SpanFilterOptionsParams,
  FeedMessagesParams,
  FeedSpansParams,
} from "../types";

/** Extract filter params by omitting pagination fields for comparison */
function omitPagination<T extends { page?: number; limit?: number }>(
  params: T,
): Omit<T, "page" | "limit"> {
  const result = { ...params };
  delete (result as Record<string, unknown>).page;
  delete (result as Record<string, unknown>).limit;
  return result as Omit<T, "page" | "limit">;
}

/** Stable stringify with sorted keys for reliable equality comparison */
function stableStringify(obj: unknown): string {
  if (obj === null || typeof obj !== "object") {
    return JSON.stringify(obj);
  }
  if (Array.isArray(obj)) {
    return "[" + obj.map(stableStringify).join(",") + "]";
  }
  const keys = Object.keys(obj as Record<string, unknown>).sort();
  const pairs = keys.map(
    (k) => JSON.stringify(k) + ":" + stableStringify((obj as Record<string, unknown>)[k]),
  );
  return "{" + pairs.join(",") + "}";
}

/** Compare filter params for equality (handles key ordering) */
function filtersEqual<T>(a: T, b: T): boolean {
  return stableStringify(a) === stableStringify(b);
}

// === Traces ===
export function useTraces(
  projectId: string,
  params?: ListTracesParams,
  options?: { enabled?: boolean; refetchInterval?: number | false; staleTime?: number },
) {
  const otelClient = useOtelClient();
  return useQuery({
    queryKey: otelKeys.traces.list(projectId, params),
    queryFn: () => otelClient.listTraces(projectId, params),
    enabled: !!projectId && (options?.enabled ?? true),
    refetchInterval: options?.refetchInterval,
    staleTime: options?.staleTime,
    placeholderData: (prev, prevQuery) => {
      // Only keep previous data if only pagination changed (not filters/sorting)
      const prevParams = prevQuery
        ? extractListParams<ListTracesParams>(prevQuery.queryKey)
        : undefined;
      if (!prevParams || !params) return undefined;
      const prevFilters = omitPagination(prevParams);
      const currFilters = omitPagination(params);
      return filtersEqual(prevFilters, currFilters) ? prev : undefined;
    },
  });
}

export function useTrace(
  projectId: string,
  traceId: string,
  params?: TraceDetailParams,
  options?: { enabled?: boolean },
) {
  const otelClient = useOtelClient();
  return useQuery({
    queryKey: otelKeys.traces.detail(projectId, traceId, params),
    queryFn: () => otelClient.getTrace(projectId, traceId, params),
    enabled: !!projectId && (options?.enabled ?? !!traceId),
    staleTime: 30_000,
  });
}

export function useTraceFilterOptions(
  projectId: string,
  params?: FilterOptionsParams & { enabled?: boolean },
) {
  const otelClient = useOtelClient();
  const { enabled = true, ...queryParams } = params ?? {};

  return useQuery({
    queryKey: otelKeys.traces.filterOptions(projectId, queryParams),
    queryFn: () => otelClient.getTraceFilterOptions(projectId, queryParams),
    enabled: !!projectId && enabled,
    staleTime: 60_000,
  });
}

// === Spans ===
/** List all spans across traces (with optional filters) */
export function useSpans(
  projectId: string,
  params?: ListSpansParams,
  options?: { enabled?: boolean },
) {
  const otelClient = useOtelClient();
  return useQuery({
    queryKey: otelKeys.spans.list(projectId, params),
    queryFn: () => otelClient.listSpans(projectId, params),
    enabled: !!projectId && (options?.enabled ?? true),
    placeholderData: (prev, prevQuery) => {
      const prevParams = prevQuery
        ? extractListParams<ListSpansParams>(prevQuery.queryKey)
        : undefined;
      if (!prevParams || !params) return undefined;
      const prevFilters = omitPagination(prevParams);
      const currFilters = omitPagination(params);
      return filtersEqual(prevFilters, currFilters) ? prev : undefined;
    },
  });
}

/** Get filter options for spans (for filter dropdowns) */
export function useSpanFilterOptions(
  projectId: string,
  params?: SpanFilterOptionsParams & { enabled?: boolean },
) {
  const otelClient = useOtelClient();
  const { enabled = true, ...queryParams } = params ?? {};

  return useQuery({
    queryKey: otelKeys.spans.filterOptions(projectId, queryParams),
    queryFn: () => otelClient.getSpanFilterOptions(projectId, queryParams),
    enabled: !!projectId && enabled,
    staleTime: 60_000,
  });
}

/** List spans for a session (convenience wrapper around useSpans) */
export function useSessionSpans(
  projectId: string,
  sessionId: string,
  options?: { include_raw_span?: boolean; enabled?: boolean },
) {
  return useSpans(
    projectId,
    {
      session_id: sessionId,
      include_raw_span: options?.include_raw_span,
      limit: 500,
      order_by: "start_time:asc",
    },
    { enabled: !!sessionId && (options?.enabled ?? true) },
  );
}

/** List spans for a specific trace */
export function useTraceSpans(
  projectId: string,
  traceId: string,
  params?: ListSpansParams,
  options?: { enabled?: boolean },
) {
  const otelClient = useOtelClient();
  return useQuery({
    queryKey: otelKeys.spans.traceList(projectId, traceId, params),
    queryFn: () => otelClient.listTraceSpans(projectId, traceId, params),
    enabled: !!projectId && (options?.enabled ?? !!traceId),
    placeholderData: (prev, prevQuery) => {
      const prevParams = prevQuery
        ? extractTraceListParams<ListSpansParams>(prevQuery.queryKey)
        : undefined;
      if (!prevParams || !params) return undefined;
      const prevFilters = omitPagination(prevParams);
      const currFilters = omitPagination(params);
      return filtersEqual(prevFilters, currFilters) ? prev : undefined;
    },
  });
}

/** Get span detail */
export function useSpan(
  projectId: string,
  traceId: string,
  spanId: string,
  params?: SpanDetailParams,
  options?: { enabled?: boolean },
) {
  const otelClient = useOtelClient();
  return useQuery({
    queryKey: otelKeys.spans.detail(projectId, traceId, spanId, params),
    queryFn: () => otelClient.getSpan(projectId, traceId, spanId, params),
    enabled: !!projectId && (options?.enabled ?? !!(traceId && spanId)),
    staleTime: 30_000,
  });
}

/** Get messages for a span */
export function useSpanMessages(
  projectId: string,
  traceId: string,
  spanId: string,
  params?: MessagesParams,
  options?: { enabled?: boolean },
) {
  const otelClient = useOtelClient();
  return useQuery({
    queryKey: otelKeys.spans.messages(projectId, traceId, spanId, params),
    queryFn: () => otelClient.getSpanMessages(projectId, traceId, spanId, params),
    enabled: !!projectId && (options?.enabled ?? !!(traceId && spanId)),
    staleTime: 30_000,
  });
}

// === Sessions ===
export function useSessions(
  projectId: string,
  params?: ListSessionsParams,
  options?: { enabled?: boolean },
) {
  const otelClient = useOtelClient();
  return useQuery({
    queryKey: otelKeys.sessions.list(projectId, params),
    queryFn: () => otelClient.listSessions(projectId, params),
    enabled: !!projectId && (options?.enabled ?? true),
    placeholderData: (prev, prevQuery) => {
      const prevParams = prevQuery
        ? extractListParams<ListSessionsParams>(prevQuery.queryKey)
        : undefined;
      if (!prevParams || !params) return undefined;
      const prevFilters = omitPagination(prevParams);
      const currFilters = omitPagination(params);
      return filtersEqual(prevFilters, currFilters) ? prev : undefined;
    },
  });
}

export function useSession(projectId: string, sessionId: string, options?: { enabled?: boolean }) {
  const otelClient = useOtelClient();
  return useQuery({
    queryKey: otelKeys.sessions.detail(projectId, sessionId),
    queryFn: () => otelClient.getSession(projectId, sessionId),
    enabled: !!projectId && (options?.enabled ?? !!sessionId),
    staleTime: 30_000,
  });
}

/** Get filter options for sessions (for filter dropdowns) */
export function useSessionFilterOptions(
  projectId: string,
  params?: FilterOptionsParams & { enabled?: boolean },
) {
  const otelClient = useOtelClient();
  const { enabled = true, ...queryParams } = params ?? {};

  return useQuery({
    queryKey: otelKeys.sessions.filterOptions(projectId, queryParams),
    queryFn: () => otelClient.getSessionFilterOptions(projectId, queryParams),
    enabled: !!projectId && enabled,
    staleTime: 60_000,
  });
}

// === Messages (Conversation History) ===
export function useTraceMessages(
  projectId: string,
  traceId: string,
  params?: MessagesParams,
  options?: { enabled?: boolean },
) {
  const otelClient = useOtelClient();
  return useQuery({
    queryKey: otelKeys.traces.messages(projectId, traceId, params),
    queryFn: () => otelClient.getTraceMessages(projectId, traceId, params),
    enabled: !!projectId && (options?.enabled ?? !!traceId),
    staleTime: 30_000,
  });
}

export function useSessionMessages(
  projectId: string,
  sessionId: string,
  params?: MessagesParams,
  options?: { enabled?: boolean },
) {
  const otelClient = useOtelClient();
  return useQuery({
    queryKey: otelKeys.sessions.messages(projectId, sessionId, params),
    queryFn: () => otelClient.getSessionMessages(projectId, sessionId, params),
    enabled: !!projectId && (options?.enabled ?? !!sessionId),
    staleTime: 30_000,
  });
}

// === Project Stats ===
export function useProjectStats(
  projectId: string,
  params: ProjectStatsParams,
  options?: { enabled?: boolean; refetchInterval?: number | false },
) {
  const otelClient = useOtelClient();
  return useQuery({
    queryKey: otelKeys.stats.query(projectId, params),
    queryFn: () => otelClient.getStats(projectId, params),
    enabled: !!projectId && (options?.enabled ?? true),
    staleTime: 10_000,
    refetchInterval: options?.refetchInterval ?? 30_000,
    placeholderData: keepPreviousData, // Keep old data visible while fetching new
  });
}

// === Feed ===
export function useFeedMessages(
  projectId: string,
  params?: FeedMessagesParams,
  options?: { enabled?: boolean },
) {
  const otelClient = useOtelClient();
  return useQuery({
    queryKey: otelKeys.feed.messages(projectId, params),
    queryFn: () => otelClient.getFeedMessages(projectId, params),
    enabled: !!projectId && (options?.enabled ?? true),
    staleTime: 0, // Always consider stale for real-time updates
  });
}

export function useFeedSpans(
  projectId: string,
  params?: FeedSpansParams,
  options?: { enabled?: boolean },
) {
  const otelClient = useOtelClient();
  return useQuery({
    queryKey: otelKeys.feed.spans(projectId, params),
    queryFn: () => otelClient.getFeedSpans(projectId, params),
    enabled: !!projectId && (options?.enabled ?? true),
    staleTime: 0, // Always consider stale for real-time updates
  });
}
