//! SQLite storage for trace indexing

pub mod events;
pub mod files;
mod migrations;
mod schema;
pub mod spans;
pub mod traces;

pub use events::EventIndex;
pub use files::ParquetFileRecord;
use migrations::run_migrations;
pub use schema::*;
pub use spans::SpanIndex;
pub use traces::TraceSummary;

use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use std::path::Path;

use crate::otel::error::OtelError;

/// SQLite store for trace metadata and indexing
pub struct SqliteStore {
    pool: SqlitePool,
}

/// Storage statistics
#[derive(Debug, Clone, Default)]
pub struct StorageStats {
    pub total_traces: u64,
    pub total_spans: u64,
    pub total_parquet_bytes: u64,
    pub total_parquet_files: u64,
}

impl SqliteStore {
    /// Open or create the SQLite database
    pub async fn open(path: &Path) -> Result<Self, OtelError> {
        let url = format!("sqlite:{}?mode=rwc", path.display());

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&url)
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to open database: {}", e)))?;

        // Enable WAL mode and optimize for write throughput
        sqlx::query("PRAGMA journal_mode = WAL")
            .execute(&pool)
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to set WAL mode: {}", e)))?;
        sqlx::query("PRAGMA synchronous = NORMAL")
            .execute(&pool)
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to set synchronous: {}", e)))?;
        sqlx::query("PRAGMA cache_size = -64000") // 64MB cache
            .execute(&pool)
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to set cache_size: {}", e)))?;
        sqlx::query("PRAGMA temp_store = MEMORY")
            .execute(&pool)
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to set temp_store: {}", e)))?;
        // Auto-checkpoint when WAL reaches ~4MB (1000 pages * 4KB)
        sqlx::query("PRAGMA wal_autocheckpoint = 1000").execute(&pool).await.map_err(|e| {
            OtelError::StorageError(format!("Failed to set wal_autocheckpoint: {}", e))
        })?;

        // Run migrations
        run_migrations(&pool).await?;

        Ok(Self { pool })
    }

    /// Get the connection pool
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Run WAL checkpoint to merge WAL into main database
    /// Called periodically and on shutdown for clean WAL rotation
    pub async fn checkpoint(&self) -> Result<(), OtelError> {
        // TRUNCATE mode: checkpoint and truncate WAL to zero bytes
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&self.pool)
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to checkpoint WAL: {}", e)))?;
        tracing::debug!("WAL checkpoint completed");
        Ok(())
    }

    /// Get storage statistics
    pub async fn get_stats(&self) -> Result<StorageStats, OtelError> {
        let row = sqlx::query_as::<_, (i64, i64, i64, i64)>(
            "SELECT total_traces, total_spans, total_parquet_bytes, total_parquet_files FROM storage_stats WHERE id = 1"
        )
        .fetch_optional(&self.pool)
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

    #[test]
    fn test_storage_stats_debug() {
        let stats = StorageStats {
            total_traces: 10,
            total_spans: 50,
            total_parquet_bytes: 1000,
            total_parquet_files: 2,
        };
        let debug = format!("{:?}", stats);
        assert!(debug.contains("total_traces: 10"));
        assert!(debug.contains("total_spans: 50"));
        assert!(debug.contains("total_parquet_bytes: 1000"));
        assert!(debug.contains("total_parquet_files: 2"));
    }

    #[test]
    fn test_storage_stats_large_values() {
        let stats = StorageStats {
            total_traces: u64::MAX,
            total_spans: u64::MAX,
            total_parquet_bytes: u64::MAX,
            total_parquet_files: u64::MAX,
        };
        assert_eq!(stats.total_traces, u64::MAX);
        assert_eq!(stats.total_spans, u64::MAX);
    }
}
