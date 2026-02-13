//! OTEL-specific DTOs for API responses

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::data::types::{MessageCategory, SpanRow};
use crate::domain::sideml::{BlockEntry, ChatRole, ContentBlock, FinishReason};

/// Helper for query params that accept string or array
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum StringOrArray {
    Single(String),
    Multiple(Vec<String>),
}

impl StringOrArray {
    pub fn into_vec(self) -> Vec<String> {
        match self {
            Self::Single(s) => vec![s],
            Self::Multiple(v) => v,
        }
    }
}

// --- Trace DTOs ---

#[derive(Debug, Serialize, ToSchema)]
pub struct TraceSummaryDto {
    pub trace_id: String,
    pub trace_name: Option<String>,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub duration_ms: Option<i64>,
    pub session_id: Option<String>,
    pub user_id: Option<String>,
    pub environment: Option<String>,
    pub span_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub input_cost: f64,
    pub output_cost: f64,
    pub cache_read_cost: f64,
    pub cache_write_cost: f64,
    pub reasoning_cost: f64,
    pub total_cost: f64,
    pub tags: Vec<String>,
    pub observation_count: i64,
    pub metadata: Option<serde_json::Value>,
    pub input_preview: Option<String>,
    pub output_preview: Option<String>,
    pub has_error: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TraceDetailDto {
    #[serde(flatten)]
    pub summary: TraceSummaryDto,
    pub spans: Vec<SpanDetailDto>,
}

// --- Span DTOs ---

#[derive(Debug, Serialize, ToSchema)]
pub struct SpanSummaryDto {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub span_name: String,
    pub span_kind: Option<String>,
    pub span_category: Option<String>,
    pub observation_type: Option<String>,
    pub framework: Option<String>,
    pub status_code: Option<String>,
    pub timestamp_start: DateTime<Utc>,
    pub timestamp_end: Option<DateTime<Utc>>,
    pub duration_ms: Option<i64>,
    pub environment: Option<String>,
    pub session_id: Option<String>,
    pub user_id: Option<String>,
    // GenAI fields
    pub model: Option<String>,
    pub gen_ai_system: Option<String>,
    pub agent_name: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub finish_reasons: Vec<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub input_cost: f64,
    pub output_cost: f64,
    pub cache_read_cost: f64,
    pub cache_write_cost: f64,
    pub reasoning_cost: f64,
    pub total_cost: f64,
    pub event_count: i64,
    pub link_count: i64,
    pub input_preview: Option<String>,
    pub output_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_span: Option<serde_json::Value>,
}

impl SpanSummaryDto {
    pub fn from_row(
        row: &SpanRow,
        event_count: i64,
        link_count: i64,
        include_raw_span: bool,
    ) -> Self {
        Self {
            trace_id: row.trace_id.clone(),
            span_id: row.span_id.clone(),
            parent_span_id: row.parent_span_id.clone(),
            span_name: row.span_name.clone().unwrap_or_default(),
            span_kind: row.span_kind.clone(),
            span_category: row.span_category.clone(),
            observation_type: row.observation_type.clone(),
            framework: row.framework.clone(),
            status_code: row.status_code.clone(),
            timestamp_start: row.timestamp_start,
            timestamp_end: row.timestamp_end,
            duration_ms: row.duration_ms,
            environment: row.environment.clone(),
            session_id: row.session_id.clone(),
            user_id: row.user_id.clone(),
            model: row.gen_ai_request_model.clone(),
            gen_ai_system: row.gen_ai_system.clone(),
            agent_name: row.gen_ai_agent_name.clone(),
            finish_reasons: row.gen_ai_finish_reasons.clone(),
            input_tokens: row.gen_ai_usage_input_tokens,
            output_tokens: row.gen_ai_usage_output_tokens,
            total_tokens: row.gen_ai_usage_total_tokens,
            cache_read_tokens: row.gen_ai_usage_cache_read_tokens,
            cache_write_tokens: row.gen_ai_usage_cache_write_tokens,
            reasoning_tokens: row.gen_ai_usage_reasoning_tokens,
            input_cost: row.gen_ai_cost_input,
            output_cost: row.gen_ai_cost_output,
            cache_read_cost: row.gen_ai_cost_cache_read,
            cache_write_cost: row.gen_ai_cost_cache_write,
            reasoning_cost: row.gen_ai_cost_reasoning,
            total_cost: row.gen_ai_cost_total,
            event_count,
            link_count,
            input_preview: row.input_preview.clone(),
            output_preview: row.output_preview.clone(),
            raw_span: if include_raw_span {
                row.raw_span
                    .as_ref()
                    .and_then(|s| serde_json::from_str(s).ok())
            } else {
                None
            },
        }
    }
}

/// Span detail response.
///
/// All span data is available via `raw_span` when `?include_raw_span=true` is passed.
#[derive(Debug, Serialize, ToSchema)]
pub struct SpanDetailDto {
    #[serde(flatten)]
    pub summary: SpanSummaryDto,
}

// --- Session DTOs ---

#[derive(Debug, Serialize, ToSchema)]
pub struct SessionSummaryDto {
    pub session_id: String,
    pub user_id: Option<String>,
    pub environment: Option<String>,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub trace_count: i64,
    pub span_count: i64,
    pub observation_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub input_cost: f64,
    pub output_cost: f64,
    pub cache_read_cost: f64,
    pub cache_write_cost: f64,
    pub reasoning_cost: f64,
    pub total_cost: f64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SessionDetailDto {
    #[serde(flatten)]
    pub summary: SessionSummaryDto,
    pub traces: Vec<TraceInSessionDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TraceInSessionDto {
    pub trace_id: String,
    pub trace_name: Option<String>,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub duration_ms: Option<i64>,
    pub total_tokens: i64,
    pub reasoning_tokens: i64,
    pub total_cost: f64,
    pub tags: Vec<String>,
}

// --- Block DTOs ---

/// A single flattened content block with comprehensive metadata.
/// Each block contains exactly ONE ContentBlock.
#[derive(Debug, Serialize, ToSchema)]
pub struct BlockDto {
    // Content
    pub entry_type: String,
    pub content: ContentBlock,
    pub role: ChatRole,

    // Position
    pub trace_id: String,
    pub span_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub message_index: i32,
    pub entry_index: i32,

    // Hierarchy
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    pub span_path: Vec<String>,

    // Timing
    pub timestamp: DateTime<Utc>,

    // Span context
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation_type: Option<String>,

    // Generation context
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,

    // Message context
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,

    // Tool context
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,

    // Metrics
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,

    // Status
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<String>,
    pub is_error: bool,

    // Source info
    pub source_type: String,
    /// Message category for semantic filtering
    pub category: MessageCategory,

    // For deduplication
    pub content_hash: String,
    pub is_semantic: bool,
}

impl BlockDto {
    pub fn from_block_entry(entry: &BlockEntry) -> Self {
        Self {
            entry_type: entry.entry_type.clone(),
            content: entry.content.clone(),
            role: entry.role,
            trace_id: entry.trace_id.clone(),
            span_id: entry.span_id.clone(),
            session_id: entry.session_id.clone(),
            message_index: entry.message_index,
            entry_index: entry.entry_index,
            parent_span_id: entry.parent_span_id.clone(),
            span_path: entry.span_path.clone(),
            timestamp: entry.timestamp,
            observation_type: entry.observation_type.clone(),
            model: entry.model.clone(),
            provider: entry.provider.clone(),
            name: entry.name.clone(),
            finish_reason: entry.finish_reason,
            tool_use_id: entry.tool_use_id.clone(),
            tool_name: entry.tool_name.clone(),
            tokens: entry.tokens,
            cost: entry.cost,
            status_code: entry.status_code.clone(),
            is_error: entry.is_error,
            source_type: entry.source_type.clone(),
            category: entry.category,
            content_hash: entry.content_hash.clone(),
            is_semantic: entry.is_semantic,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MessagesMetadataDto {
    pub total_messages: i64,
    pub total_tokens: i64,
    pub total_cost: f64,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MessagesResponseDto {
    pub messages: Vec<BlockDto>,
    pub metadata: MessagesMetadataDto,
    /// Deduplicated tool definitions sorted by name
    pub tool_definitions: Vec<serde_json::Value>,
    /// Deduplicated tool names sorted alphabetically
    pub tool_names: Vec<String>,
}

// --- Project Stats DTOs ---

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProjectStatsDto {
    pub period: PeriodDto,
    pub counts: CountsDto,
    pub costs: CostsDto,
    pub tokens: TokensDto,
    pub by_framework: Vec<FrameworkBreakdownDto>,
    pub by_model: Vec<ModelBreakdownDto>,
    pub recent_activity_count: i64,
    pub avg_trace_duration_ms: Option<f64>,
    pub trend_data: Vec<TrendBucketDto>,
    pub latency_trend_data: Vec<LatencyBucketDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PeriodDto {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CountsDto {
    pub traces: i64,
    pub traces_previous: i64,
    pub sessions: i64,
    pub spans: i64,
    pub unique_users: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CostsDto {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub reasoning: f64,
    pub total: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TokensDto {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub reasoning: i64,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FrameworkBreakdownDto {
    pub framework: Option<String>,
    pub count: i64,
    pub percentage: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ModelBreakdownDto {
    pub model: Option<String>,
    pub tokens: i64,
    pub cost: f64,
    pub percentage: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TrendBucketDto {
    pub bucket: DateTime<Utc>,
    pub tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LatencyBucketDto {
    pub bucket: DateTime<Utc>,
    pub avg_duration_ms: f64,
}

// --- Feed DTOs ---

/// Pagination info for feed endpoints
#[derive(Debug, Serialize, ToSchema)]
pub struct FeedPagination {
    /// Cursor for the next page (base64 encoded)
    pub next_cursor: Option<String>,
    /// Whether more results exist
    pub has_more: bool,
}

/// Metadata for feed messages response
#[derive(Debug, Serialize, ToSchema)]
pub struct FeedMessagesMetadata {
    /// Number of messages in this response
    pub message_count: u32,
    /// Number of unique spans contributing messages
    pub span_count: u32,
    /// Total tokens from contributing spans
    pub total_tokens: i64,
    /// Total cost from contributing spans
    pub total_cost: f64,
}

/// Feed messages response with cursor-based pagination
#[derive(Debug, Serialize, ToSchema)]
pub struct FeedMessagesResponse {
    /// Blocks sorted by span timestamp DESC
    pub data: Vec<BlockDto>,
    /// Pagination information
    pub pagination: FeedPagination,
    /// Response metadata
    pub metadata: FeedMessagesMetadata,
    /// Deduplicated tool definitions
    pub tool_definitions: Vec<serde_json::Value>,
    /// Deduplicated tool names
    pub tool_names: Vec<String>,
}

/// Feed spans response with cursor-based pagination
#[derive(Debug, Serialize, ToSchema)]
pub struct FeedSpansResponse {
    /// Spans sorted by ingested_at DESC
    pub data: Vec<SpanSummaryDto>,
    /// Pagination information
    pub pagination: FeedPagination,
}
