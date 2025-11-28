//! SQLite storage operations for trace indexing
//!
//! This module provides OTEL-specific database operations.
//! Schema and migrations are managed by the global sqlite module.

pub mod attributes;
pub mod events;
pub mod files;
pub mod sessions;
pub mod spans;
pub mod traces;

pub use attributes::{
    AttributeKey, AttributeKeyCache, AttributeValue, create_attribute_cache,
    get_all_attribute_keys, get_attribute_distinct_values, get_span_attributes,
    get_trace_attributes, insert_span_attributes_batch, insert_span_attributes_batch_with_tx,
    insert_trace_attributes_batch, insert_trace_attributes_batch_with_tx,
};
pub use events::EventIndex;
pub use files::ParquetFileRecord;
pub use sessions::{
    SessionSummary, get_session, list_sessions, soft_delete_session, upsert_sessions_batch_with_tx,
};
pub use spans::SpanIndex;
pub use traces::TraceSummary;

use sqlx::SqlitePool;

use crate::otel::error::OtelError;

/// Storage statistics
#[derive(Debug, Clone, Default)]
pub struct StorageStats {
    pub total_traces: u64,
    pub total_spans: u64,
    pub total_parquet_bytes: u64,
    pub total_parquet_files: u64,
}

/// Get storage statistics from the database
pub async fn get_storage_stats(pool: &SqlitePool) -> Result<StorageStats, OtelError> {
    let row = sqlx::query_as::<_, (i64, i64, i64, i64)>(
        "SELECT total_traces, total_spans, total_parquet_bytes, total_parquet_files FROM storage_stats WHERE id = 1"
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to get stats: {}", e)))?;

    Ok(row
        .map(|r| StorageStats {
            total_traces: r.0 as u64,
            total_spans: r.1 as u64,
            total_parquet_bytes: r.2 as u64,
            total_parquet_files: r.3 as u64,
        })
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_stats_default() {
        let stats = StorageStats::default();
        assert_eq!(stats.total_traces, 0);
        assert_eq!(stats.total_spans, 0);
        assert_eq!(stats.total_parquet_bytes, 0);
        assert_eq!(stats.total_parquet_files, 0);
    }

    #[test]
    fn test_storage_stats_clone() {
        let stats = StorageStats {
            total_traces: 100,
            total_spans: 500,
            total_parquet_bytes: 1024 * 1024,
            total_parquet_files: 5,
        };
        let cloned = stats.clone();
        assert_eq!(cloned.total_traces, 100);
        assert_eq!(cloned.total_spans, 500);
        assert_eq!(cloned.total_parquet_bytes, 1024 * 1024);
        assert_eq!(cloned.total_parquet_files, 5);
    }
}
