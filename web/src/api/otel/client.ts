import type { ApiClient } from "../api-client";
import type {
  FilterOptionsParams,
  FilterOptionsResponse,
  ListSessionsParams,
  ListSpansParams,
  ListTracesParams,
  SpanDetailParams,
  SpanFilterOptionsParams,
  TraceDetailParams,
  MessagesParams,
  MessagesResponse,
  PaginatedResponse,
  ProjectStats,
  ProjectStatsParams,
  SessionDetail,
  SessionSummary,
  SpanDetail,
  SpanSummary,
  SseParams,
  SSEHandlers,
  TraceDetail,
  TraceSummary,
  FeedMessagesParams,
  FeedMessagesResponse,
  FeedSpansParams,
  FeedSpansResponse,
} from "./types";

export class OtelClient {
  private client: ApiClient;

  constructor(client: ApiClient) {
    this.client = client;
  }

  /** Build project-scoped base path */
  private basePath(projectId: string): string {
    return `/project/${projectId}/otel`;
  }

  /** Expose buildQueryString for SSE subscriptions */
  buildQueryString(params: Record<string, unknown>): string {
    return this.client.buildQueryString(params);
  }

  // === Traces ===
  async listTraces(
    projectId: string,
    params?: ListTracesParams,
  ): Promise<PaginatedResponse<TraceSummary>> {
    return this.client.get<PaginatedResponse<TraceSummary>>(
      `${this.basePath(projectId)}/traces`,
      params as Record<string, unknown>,
    );
  }

  async getTrace(
    projectId: string,
    traceId: string,
    params?: TraceDetailParams,
  ): Promise<TraceDetail> {
    return this.client.get<TraceDetail>(
      `${this.basePath(projectId)}/traces/${traceId}`,
      params as Record<string, unknown>,
    );
  }

  async getTraceFilterOptions(
    projectId: string,
    params?: FilterOptionsParams,
  ): Promise<FilterOptionsResponse> {
    return this.client.get<FilterOptionsResponse>(
      `${this.basePath(projectId)}/traces/filter-options`,
      params as Record<string, unknown>,
    );
  }

  // === Spans ===
  /** List all spans across traces (with optional filters) */
  async listSpans(
    projectId: string,
    params?: ListSpansParams,
  ): Promise<PaginatedResponse<SpanSummary>> {
    return this.client.get<PaginatedResponse<SpanSummary>>(
      `${this.basePath(projectId)}/spans`,
      params as Record<string, unknown>,
    );
  }

  /** Get filter options for spans (for filter dropdowns) */
  async getSpanFilterOptions(
    projectId: string,
    params?: SpanFilterOptionsParams,
  ): Promise<FilterOptionsResponse> {
    return this.client.get<FilterOptionsResponse>(
      `${this.basePath(projectId)}/spans/filter-options`,
      params as Record<string, unknown>,
    );
  }

  /** List spans for a specific trace */
  async listTraceSpans(
    projectId: string,
    traceId: string,
    params?: ListSpansParams,
  ): Promise<PaginatedResponse<SpanSummary>> {
    return this.client.get<PaginatedResponse<SpanSummary>>(
      `${this.basePath(projectId)}/traces/${traceId}/spans`,
      params as Record<string, unknown>,
    );
  }

  /** Get span detail */
  async getSpan(
    projectId: string,
    traceId: string,
    spanId: string,
    params?: SpanDetailParams,
  ): Promise<SpanDetail> {
    return this.client.get<SpanDetail>(
      `${this.basePath(projectId)}/traces/${traceId}/spans/${spanId}`,
      params as Record<string, unknown>,
    );
  }

  /** Get messages for a span */
  async getSpanMessages(
    projectId: string,
    traceId: string,
    spanId: string,
    params?: MessagesParams,
  ): Promise<MessagesResponse> {
    return this.client.get<MessagesResponse>(
      `${this.basePath(projectId)}/traces/${traceId}/spans/${spanId}/messages`,
      params as Record<string, unknown>,
    );
  }

  // === Sessions ===
  async listSessions(
    projectId: string,
    params?: ListSessionsParams,
  ): Promise<PaginatedResponse<SessionSummary>> {
    return this.client.get<PaginatedResponse<SessionSummary>>(
      `${this.basePath(projectId)}/sessions`,
      params as Record<string, unknown>,
    );
  }

  async getSession(projectId: string, sessionId: string): Promise<SessionDetail> {
    return this.client.get<SessionDetail>(`${this.basePath(projectId)}/sessions/${sessionId}`);
  }

  /** Get filter options for sessions (for filter dropdowns) */
  async getSessionFilterOptions(
    projectId: string,
    params?: FilterOptionsParams,
  ): Promise<FilterOptionsResponse> {
    return this.client.get<FilterOptionsResponse>(
      `${this.basePath(projectId)}/sessions/filter-options`,
      params as Record<string, unknown>,
    );
  }

  // === Messages (Conversation History) ===
  async getTraceMessages(
    projectId: string,
    traceId: string,
    params?: MessagesParams,
  ): Promise<MessagesResponse> {
    return this.client.get<MessagesResponse>(
      `${this.basePath(projectId)}/traces/${traceId}/messages`,
      params as Record<string, unknown>,
    );
  }

  async getSessionMessages(
    projectId: string,
    sessionId: string,
    params?: MessagesParams,
  ): Promise<MessagesResponse> {
    return this.client.get<MessagesResponse>(
      `${this.basePath(projectId)}/sessions/${sessionId}/messages`,
      params as Record<string, unknown>,
    );
  }

  // === SSE ===
  subscribeToSpans(
    projectId: string,
    params: SseParams | undefined,
    handlers: SSEHandlers,
  ): () => void {
    const queryString = this.buildQueryString((params ?? {}) as Record<string, unknown>);
    const endpoint = queryString
      ? `${this.basePath(projectId)}/sse?${queryString}`
      : `${this.basePath(projectId)}/sse`;
    return this.client.connectSSE(endpoint, handlers);
  }

  // === Delete ===
  async deleteTraces(projectId: string, traceIds: string[]): Promise<void> {
    await this.client.delete(`${this.basePath(projectId)}/traces`, { trace_ids: traceIds });
  }

  async deleteSessions(projectId: string, sessionIds: string[]): Promise<void> {
    await this.client.delete(`${this.basePath(projectId)}/sessions`, { session_ids: sessionIds });
  }

  async deleteSpans(
    projectId: string,
    spanIds: { trace_id: string; span_id: string }[],
  ): Promise<void> {
    await this.client.delete(`${this.basePath(projectId)}/spans`, { spans: spanIds });
  }

  // === Stats ===
  async getStats(projectId: string, params: ProjectStatsParams): Promise<ProjectStats> {
    return this.client.get<ProjectStats>(
      `${this.basePath(projectId)}/stats`,
      params as unknown as Record<string, unknown>,
    );
  }

  // === Feed ===
  async getFeedMessages(
    projectId: string,
    params?: FeedMessagesParams,
  ): Promise<FeedMessagesResponse> {
    return this.client.get<FeedMessagesResponse>(
      `${this.basePath(projectId)}/feed/messages`,
      params as Record<string, unknown>,
    );
  }

  async getFeedSpans(projectId: string, params?: FeedSpansParams): Promise<FeedSpansResponse> {
    return this.client.get<FeedSpansResponse>(
      `${this.basePath(projectId)}/feed/spans`,
      params as Record<string, unknown>,
    );
  }
}
