//! Shared data types for all database backends
//!
//! This module contains types that are used across multiple database backends
//! (DuckDB, ClickHouse, SQLite, PostgreSQL) to ensure consistent data models.

mod analytics;
mod enums;
mod messages;
mod normalized;
mod stats;
mod transactional;

// Re-export enum types
pub use enums::{
    AggregationTemporality, Framework, MessageCategory, MessageSourceType, MetricType,
    ObservationType, SpanCategory,
};

// Re-export normalized types (for ingestion)
pub use normalized::{NormalizedMetric, NormalizedSpan};

// Re-export analytics types (query results and params)
pub use analytics::{
    EventRow, FeedSpansParams, LinkRow, ListSessionsParams, ListSpansParams, ListTracesParams,
    ObservationTokens, SessionRow, SpanCounts, SpanRow, TraceRow, deduplicate_spans,
    filter_observations, find_root_span, get_observation_cost, get_observation_tokens,
    get_observation_type, is_observation, parse_finish_reasons, parse_tags,
};

// Re-export message types
pub use messages::{FeedMessagesParams, MessageQueryParams, MessageQueryResult, MessageSpanRow};

// Re-export stats types
pub use stats::{
    CostsResult, CountsResult, FrameworkBreakdown, LatencyBucket, ModelBreakdown,
    ProjectStatsResult, StatsParams, TokensResult, TrendBucket,
};

// Re-export transactional types (SQLite/PostgreSQL)
pub use transactional::{
    ApiKeyRow, ApiKeyScope, ApiKeyValidation, AuthMethodRow, FileRow, LastOwnerResult,
    MemberWithUser, MembershipRow, OrgWithRole, OrganizationRow, ProjectRow, UserRow,
};
