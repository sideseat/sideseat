import type {
  FilterOptionsParams,
  ListTracesParams,
  ListSpansParams,
  ListSessionsParams,
  MessagesParams,
  TraceDetailParams,
  SpanDetailParams,
  SpanFilterOptionsParams,
  ProjectStatsParams,
  FeedMessagesParams,
  FeedSpansParams,
} from "./types";

// ============================================================================
// QUERY KEY PARAM EXTRACTORS
// ============================================================================

/**
 * Extract params from a list query key (traces.list, spans.list, sessions.list).
 *
 * Query key structure: ["otel", projectId, resource, "list", params]
 *
 * @example
 * const params = extractListParams<ListTracesParams>(queryKey);
 */
export function extractListParams<T>(queryKey: readonly unknown[]): T | undefined {
  // Index 4 is where params live for list queries: ["otel", projectId, resource, "list", params]
  return queryKey[4] as T | undefined;
}

/**
 * Extract params from a trace span list query key (spans.traceList).
 *
 * Query key structure: ["otel", projectId, "spans", "traceList", traceId, params]
 *
 * @example
 * const params = extractTraceListParams<ListSpansParams>(queryKey);
 */
export function extractTraceListParams<T>(queryKey: readonly unknown[]): T | undefined {
  // Index 5 is where params live for traceList queries: ["otel", projectId, "spans", "traceList", traceId, params]
  return queryKey[5] as T | undefined;
}

export const otelKeys = {
  // Project-scoped base key
  project: (projectId: string) => ["otel", projectId] as const,

  traces: {
    all: (projectId: string) => [...otelKeys.project(projectId), "traces"] as const,
    lists: (projectId: string) => [...otelKeys.traces.all(projectId), "list"] as const,
    list: (projectId: string, params?: ListTracesParams) =>
      [...otelKeys.traces.lists(projectId), params] as const,
    detail: (projectId: string, id: string, params?: TraceDetailParams) =>
      [...otelKeys.traces.all(projectId), "detail", id, params] as const,
    filterOptions: (projectId: string, params?: FilterOptionsParams) =>
      [...otelKeys.traces.all(projectId), "filterOptions", params] as const,
    messages: (projectId: string, traceId: string, params?: MessagesParams) =>
      [...otelKeys.traces.all(projectId), "messages", traceId, params] as const,
  },
  spans: {
    all: (projectId: string) => [...otelKeys.project(projectId), "spans"] as const,
    lists: (projectId: string) => [...otelKeys.spans.all(projectId), "list"] as const,
    list: (projectId: string, params?: ListSpansParams) =>
      [...otelKeys.spans.lists(projectId), params] as const,
    traceList: (projectId: string, traceId: string, params?: ListSpansParams) =>
      [...otelKeys.spans.all(projectId), "traceList", traceId, params] as const,
    detail: (projectId: string, traceId: string, spanId: string, params?: SpanDetailParams) =>
      [...otelKeys.spans.all(projectId), "detail", traceId, spanId, params] as const,
    filterOptions: (projectId: string, params?: SpanFilterOptionsParams) =>
      [...otelKeys.spans.all(projectId), "filterOptions", params] as const,
    messages: (projectId: string, traceId: string, spanId: string, params?: MessagesParams) =>
      [...otelKeys.spans.all(projectId), "messages", traceId, spanId, params] as const,
  },
  sessions: {
    all: (projectId: string) => [...otelKeys.project(projectId), "sessions"] as const,
    lists: (projectId: string) => [...otelKeys.sessions.all(projectId), "list"] as const,
    list: (projectId: string, params?: ListSessionsParams) =>
      [...otelKeys.sessions.lists(projectId), params] as const,
    detail: (projectId: string, id: string) =>
      [...otelKeys.sessions.all(projectId), "detail", id] as const,
    filterOptions: (projectId: string, params?: FilterOptionsParams) =>
      [...otelKeys.sessions.all(projectId), "filterOptions", params] as const,
    messages: (projectId: string, sessionId: string, params?: MessagesParams) =>
      [...otelKeys.sessions.all(projectId), "messages", sessionId, params] as const,
  },
  stats: {
    all: (projectId: string) => [...otelKeys.project(projectId), "stats"] as const,
    query: (projectId: string, params: ProjectStatsParams) =>
      [...otelKeys.stats.all(projectId), params] as const,
  },
  feed: {
    all: (projectId: string) => [...otelKeys.project(projectId), "feed"] as const,
    messages: (projectId: string, params?: FeedMessagesParams) =>
      [...otelKeys.feed.all(projectId), "messages", params] as const,
    spans: (projectId: string, params?: FeedSpansParams) =>
      [...otelKeys.feed.all(projectId), "spans", params] as const,
  },
};
