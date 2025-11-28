//! Query engine for trace data

mod aggregations;
mod datafusion;
mod engine;
mod filters;
mod pagination;
mod sqlite_queries;

pub use aggregations::{
    CategoryCount, GlobalStats, TraceDurationSummary, TraceTokenSummary, get_global_stats,
    get_trace_category_breakdown, get_trace_duration_summary, get_trace_token_summary,
};
pub use datafusion::{DataFusionExecutor, FrameworkDurationStats, ModelTokenUsage};
pub use engine::QueryEngine;
pub use filters::{AttributeFilter, FilterOperator, SpanFilter, TraceFilter};
pub use pagination::{Cursor, PageResult};
pub use sqlite_queries::{
    StorageStats, count_traces_by_framework, count_traces_by_service, get_spans_by_trace_id,
    get_storage_stats, get_trace_by_id, list_traces,
};
