//! Shared data types for all database backends
//!
//! This module contains types that are used across multiple database backends
//! (DuckDB, ClickHouse, SQLite, PostgreSQL) to ensure consistent data models.

mod analytics;
mod enums;
mod logs;
mod messages;
mod metrics;
mod normalized;
pub mod order;
mod project_id;
mod search;
mod staging;
mod stats;
mod transactional;

// Result ordering: the column, the direction and the SQL they render. Lives here rather than in
// `api::types` because three analytics DTOs carry it, which made the storage layer import from HTTP.
pub use order::{OrderBy, OrderDirection};
pub use project_id::ProjectId;

// Re-export enum types
pub use enums::{
    AggregationTemporality, MessageCategory, MessageSourceType, MetricType, ObservationType,
    SpanCategory,
};
// No framework enum here. It is the detection oracle's vocabulary, not a shape any port speaks in, and a
// test-only item cannot be seen by a dependent crate - so it lives beside its only users, in the domain.

// Re-export normalized types (for ingestion)
pub use normalized::{NormalizedLog, NormalizedMetric, NormalizedSpan, json_to_pre_serialized};
pub use search::{
    DEFAULT_SEARCH_MAX_EXAMINED, LogSearchSource, SEARCH_RECALL_FLOOR, SEARCH_TERMS_PER_FIELD,
    SearchBackfillDocument, SearchBackfillSource, SearchCandidate, SearchCursor, SearchDocument,
    SearchExpr, SearchField, SearchFieldTerms, SearchLogRecord, SearchPage, SearchQuery,
    SearchRecord, SearchRecordId, SearchSignal, SearchSource, SearchSpanRecord, SpanSearchSource,
};

// Re-export analytics types (query results and params)
pub use analytics::{
    EventRow, FeedSpansParams, LinkRow, ListSessionsParams, ListSpansParams, ListTracesParams,
    ObservationTokens, PressureSpanCandidate, SessionRow, SpanCounts, SpanIdentity, SpanRow,
    TraceRow, deduplicate_by_span_identity, filter_observations, find_root_span,
    get_observation_cost, get_observation_tokens, get_observation_type, is_observation,
    parse_finish_reasons, parse_tags,
};

// Re-export message types
pub use logs::{ListLogsParams, LogRow};
pub use messages::{
    FeedMessagesParams, MessageQueryParams, MessageQueryResult, MessageSpanRow,
    SESSION_FILTER_OPTION_COLUMNS, SPAN_FILTER_OPTION_COLUMNS, TRACE_FILTER_OPTION_COLUMNS,
};
pub use metrics::{ListMetricsParams, MetricAggregateRow, MetricRow};

// Re-export stats types
pub use staging::{StagedPayload, StagedRecord, StagedSignal};
pub use stats::{
    CostsResult, CountsResult, FrameworkBreakdown, LatencyBucket, ModelBreakdown,
    ProjectStatsResult, StatsParams, TokensResult, TrendBucket,
};

// Re-export transactional types (SQLite/PostgreSQL)
pub use transactional::{
    ApiKeyRow, ApiKeyScope, ApiKeyValidation, AuthMethodRow, ContentBodyBackfillProgress,
    ContentBodyObject, CredentialPermissionRow, CredentialRow, FileRow, LastOwnerResult,
    MemberWithUser, MembershipRow, OrgWithRole, OrganizationRow, ProjectHold, ProjectRow,
    ProjectStorageUsage, SpanBodyAssociation, SpanBodyField, SpanBodySource, UserRow,
};
