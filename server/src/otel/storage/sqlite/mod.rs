//! SQLite storage operations for trace indexing
//!
//! This module provides OTEL-specific database operations.
//! Schema and migrations are managed by the global sqlite module.

pub mod attributes;
pub mod events;
pub mod sessions;
pub mod spans;
pub mod traces;

pub use attributes::{
    AttributeKey, AttributeKeyCache, AttributeValue, create_attribute_cache,
    get_all_attribute_keys, get_attribute_distinct_values, get_trace_attributes,
    insert_span_attributes_batch_with_tx, insert_trace_attributes_batch_with_tx,
};
pub use events::{EventIndex, get_events_by_span, insert_events_batch_with_tx};
pub use sessions::{
    SessionSummary, delete_session, get_session, list_sessions, upsert_sessions_batch_with_tx,
};
pub use spans::{SpanIndex, get_span_by_id};
pub use traces::TraceSummary;

use sqlx::SqlitePool;

use crate::otel::error::OtelError;

/// Storage statistics
#[derive(Debug, Clone, Default)]
pub struct StorageStats {
    pub total_traces: u64,
    pub total_spans: u64,
}

/// Get storage statistics by querying actual tables
pub async fn get_storage_stats(pool: &SqlitePool) -> Result<StorageStats, OtelError> {
    let total_traces: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM traces")
        .fetch_one(pool)
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to count traces: {}", e)))?;

    let total_spans: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM spans")
            .fetch_one(pool)
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to count spans: {}", e)))?;

    Ok(StorageStats { total_traces: total_traces as u64, total_spans: total_spans as u64 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_stats_default() {
        let stats = StorageStats::default();
        assert_eq!(stats.total_traces, 0);
        assert_eq!(stats.total_spans, 0);
    }

    #[test]
    fn test_storage_stats_clone() {
        let stats = StorageStats { total_traces: 100, total_spans: 500 };
        let cloned = stats.clone();
        assert_eq!(cloned.total_traces, 100);
        assert_eq!(cloned.total_spans, 500);
    }
}
