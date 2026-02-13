/**
 * Otel Domain Types
 *
 * TypeScript interfaces for OpenTelemetry domain entities.
 */

// === Pagination ===

export interface PaginationMeta {
  page: number;
  limit: number;
  total_items: number;
  total_pages: number;
}

export interface PaginatedResponse<T> {
  data: T[];
  meta: PaginationMeta;
}

// === Traces ===

export interface TraceSummary {
  trace_id: string;
  trace_name: string | null;
  start_time: string;
  end_time: string | null;
  duration_ms: number | null;
  session_id: string | null;
  user_id: string | null;
  environment: string | null;
  span_count: number;
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  reasoning_tokens: number;
  input_cost: number;
  output_cost: number;
  cache_read_cost: number;
  cache_write_cost: number;
  reasoning_cost: number;
  total_cost: number;
  tags: string[];
  observation_count: number;
  metadata: Record<string, unknown> | null;
  input_preview: string | null;
  output_preview: string | null;
  has_error: boolean;
}

export interface TraceDetail extends TraceSummary {
  spans: SpanDetail[];
}

// === Spans ===

export interface SpanSummary {
  trace_id: string;
  span_id: string;
  parent_span_id: string | null;
  span_name: string;
  span_kind: string | null;
  span_category: string | null;
  observation_type: string | null;
  framework: string | null;
  status_code: string | null;
  timestamp_start: string;
  timestamp_end: string | null;
  duration_ms: number | null;
  environment: string | null;
  session_id: string | null;
  user_id: string | null;
  model: string | null;
  gen_ai_system: string | null;
  agent_name: string | null;
  finish_reasons?: string[];
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  reasoning_tokens: number;
  input_cost: number;
  output_cost: number;
  cache_read_cost: number;
  cache_write_cost: number;
  reasoning_cost: number;
  total_cost: number;
  event_count: number;
  link_count: number;
  input_preview: string | null;
  output_preview: string | null;
  /** Raw OTLP span JSON (only present when include_raw_span=true) */
  raw_span?: Record<string, unknown>;
}

/** SpanDetail is now the same as SpanSummary (all extracted fields removed, use raw_span for full data) */
export type SpanDetail = SpanSummary;

/**
 * Message categories matching server/src/data/types/enums.rs
 * Used for semantic filtering of messages
 */
export type MessageCategory =
  | "Log"
  | "Exception"
  | "GenAISystemMessage"
  | "GenAIUserMessage"
  | "GenAIAssistantMessage"
  | "GenAIToolMessage"
  | "GenAIToolInput"
  | "GenAIToolDefinitions"
  | "GenAIChoice"
  | "GenAIContext"
  | "Retrieval"
  | "Observation"
  | "Other";

/**
 * ContentBlock types matching server/src/domain/sideml/types.rs
 * All 15 content block types for multimodal messages
 */
export type ContentBlock =
  | { type: "text"; text: string }
  | { type: "image"; media_type?: string; source: string; data: string; detail?: string }
  | { type: "audio"; media_type?: string; source: string; data: string }
  | { type: "document"; media_type?: string; name?: string; source: string; data: string }
  | { type: "video"; media_type?: string; source: string; data: string }
  | { type: "file"; media_type?: string; name?: string; source: string; data: string }
  | { type: "tool_use"; id?: string; name: string; input: Record<string, unknown> }
  | { type: "tool_result"; tool_use_id?: string; content: unknown; is_error?: boolean }
  | { type: "tool_definitions"; tools: unknown[]; tool_choice?: unknown }
  | { type: "context"; data: unknown; context_type?: string }
  | { type: "refusal"; message: string }
  | { type: "json"; data: unknown }
  | { type: "thinking"; text: string; signature?: string }
  | { type: "redacted_thinking"; data: string }
  | { type: "unknown" };

// === Sessions ===

export interface SessionSummary {
  session_id: string;
  user_id: string | null;
  environment: string | null;
  start_time: string;
  end_time: string | null;
  trace_count: number;
  span_count: number;
  observation_count: number;
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  reasoning_tokens: number;
  input_cost: number;
  output_cost: number;
  cache_read_cost: number;
  cache_write_cost: number;
  reasoning_cost: number;
  total_cost: number;
}

export interface SessionDetail extends SessionSummary {
  traces: TraceInSession[];
}

export interface TraceInSession {
  trace_id: string;
  trace_name: string | null;
  start_time: string;
  end_time: string | null;
  duration_ms: number | null;
  total_tokens: number;
  reasoning_tokens: number;
  total_cost: number;
  tags: string[];
}

// === SSE ===

export interface SseSpanEvent {
  project_id: string | null;
  trace_id: string;
  span_id: string;
  session_id: string | null;
  user_id: string | null;
}

export interface SSEHandlers {
  onSpan: (event: SseSpanEvent) => void;
  onError?: (error: Error) => void;
  onOpen?: () => void;
  onClose?: () => void;
}

export interface SseParams {
  trace_id?: string;
  span_id?: string;
  session_id?: string;
}

// === Filters ===

export type FilterType = "datetime" | "string" | "number" | "string_options" | "boolean" | "null";
export type FilterOperator =
  | "="
  | "<>"
  | ">"
  | "<"
  | ">="
  | "<="
  | "contains"
  | "starts_with"
  | "ends_with"
  | "any of"
  | "none of"
  | "is null"
  | "is not null";

export interface Filter {
  type: FilterType;
  column: string;
  operator: FilterOperator;
  value: string | number | boolean | string[] | null;
}

// === Query Parameters ===

export interface ListTracesParams {
  page?: number;
  limit?: number;
  order_by?: string;
  session_id?: string;
  user_id?: string;
  environment?: string | string[];
  from_timestamp?: string;
  to_timestamp?: string;
  filters?: Filter[];
  include_nongenai?: boolean;
}

export interface ListSpansParams {
  page?: number;
  limit?: number;
  order_by?: string;
  trace_id?: string;
  session_id?: string;
  user_id?: string;
  environment?: string | string[];
  span_category?: string;
  observation_type?: string;
  framework?: string;
  gen_ai_request_model?: string;
  status_code?: string;
  from_timestamp?: string;
  to_timestamp?: string;
  filters?: Filter[];
  include_raw_span?: boolean;
  /** Filter to observations only (spans with observation_type OR gen_ai_request_model) */
  is_observation?: boolean;
}

export interface ListSessionsParams {
  page?: number;
  limit?: number;
  order_by?: string;
  user_id?: string;
  environment?: string | string[];
  from_timestamp?: string;
  to_timestamp?: string;
  filters?: Filter[];
}

export interface TraceDetailParams {
  include_raw_span?: boolean;
}

export interface SpanDetailParams {
  include_raw_span?: boolean;
}

// === Filter Options ===

export interface FilterOption {
  value: string;
  count: number;
}

export interface FilterOptionsResponse {
  options: Record<string, FilterOption[]>;
}

export interface FilterOptionsParams {
  columns?: string;
  from_timestamp?: string;
  to_timestamp?: string;
}

export interface SpanFilterOptionsParams extends FilterOptionsParams {
  observations_only?: boolean;
}

// === Blocks (Flattened Content) ===

/**
 * A single flattened content block with comprehensive metadata.
 * Each block contains exactly ONE ContentBlock.
 * Matches server's BlockDto.
 */
export interface Block {
  // Content
  entry_type: string;
  content: ContentBlock;
  role: string;

  // Position
  trace_id: string;
  span_id: string;
  session_id?: string;
  message_index: number;
  entry_index: number;

  // Hierarchy
  parent_span_id?: string;
  span_path: string[];

  // Timing
  timestamp: string;

  // Span context
  observation_type?: string;

  // Generation context
  model?: string;
  provider?: string;

  // Message context
  name?: string;
  finish_reason?: string;

  // Tool context
  tool_use_id?: string;
  tool_name?: string;

  // Metrics
  tokens?: number;
  cost?: number;

  // Status
  status_code?: string;
  is_error: boolean;

  // Source info
  source_type: "event" | "attribute";
  /** Message category for semantic filtering */
  category: MessageCategory;

  // For deduplication
  content_hash: string;
  is_semantic: boolean;
}

export interface MessagesMetadata {
  total_messages: number;
  total_tokens: number;
  total_cost: number;
  start_time: string;
  end_time: string | null;
}

export interface MessagesResponse {
  messages: Block[];
  metadata: MessagesMetadata;
  tool_definitions: Record<string, unknown>[];
  tool_names: string[];
}

export interface MessagesParams {
  from_timestamp?: string;
  to_timestamp?: string;
  role?: string;
}

// === Project Stats ===

export interface ProjectStats {
  period: {
    from: string;
    to: string;
  };
  counts: {
    traces: number;
    traces_previous: number;
    sessions: number;
    spans: number;
    unique_users: number;
  };
  costs: {
    input: number;
    output: number;
    cache_read: number;
    cache_write: number;
    reasoning: number;
    total: number;
  };
  tokens: {
    input: number;
    output: number;
    cache_read: number;
    cache_write: number;
    reasoning: number;
    total: number;
  };
  by_framework: Array<{
    framework: string | null;
    count: number;
    percentage: number;
  }>;
  by_model: Array<{
    model: string | null;
    tokens: number;
    cost: number;
    percentage: number;
  }>;
  recent_activity_count: number;
  avg_trace_duration_ms: number | null;
  trend_data: Array<{
    bucket: string;
    tokens: number;
  }>;
  latency_trend_data: Array<{
    bucket: string;
    avg_duration_ms: number;
  }>;
}

export interface ProjectStatsParams {
  from_timestamp: string;
  to_timestamp: string;
  timezone?: string;
}

// === Feed API ===

export interface FeedPagination {
  next_cursor: string | null;
  has_more: boolean;
}

export interface FeedMessagesMetadata {
  message_count: number;
  span_count: number;
  total_tokens: number;
  total_cost: number;
}

export interface FeedMessagesResponse {
  data: Block[];
  pagination: FeedPagination;
  metadata: FeedMessagesMetadata;
  tool_definitions: Record<string, unknown>[];
  tool_names: string[];
}

export interface FeedSpansResponse {
  data: SpanSummary[];
  pagination: FeedPagination;
}

export interface FeedMessagesParams {
  limit?: number;
  cursor?: string;
  start_time?: string;
  end_time?: string;
  role?: string;
}

export interface FeedSpansParams {
  limit?: number;
  cursor?: string;
  start_time?: string;
  end_time?: string;
  is_observation?: boolean;
  include_raw_span?: boolean;
}
