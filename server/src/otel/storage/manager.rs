//! Trace storage manager (coordinates SQLite and Parquet)

use std::path::PathBuf;
use std::sync::Arc;
use tracing::debug;

use super::parquet::ParquetWriterPool;
use super::sqlite::SqliteStore;
use crate::core::config::OtelConfig;
use crate::otel::error::OtelError;

/// Trace storage manager
pub struct TraceStorageManager {
    traces_dir: PathBuf,
    sqlite: Arc<SqliteStore>,
    parquet_pool: Arc<ParquetWriterPool>,
}

impl TraceStorageManager {
    /// Initialize the trace storage manager
    /// Note: data_dir should already be the traces directory (e.g., from DataSubdir::Traces)
    pub async fn init(data_dir: PathBuf, config: &OtelConfig) -> Result<Self, OtelError> {
        let traces_dir = data_dir.clone();
        tokio::fs::create_dir_all(&traces_dir)
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to create traces dir: {}", e)))?;

        let db_path = traces_dir.join("traces.db");

        debug!("Initializing trace storage at {:?}", traces_dir);

        // Initialize SQLite store
        let sqlite = SqliteStore::open(&db_path).await?;

        // Initialize Parquet writer pool
        let parquet_pool = ParquetWriterPool::new(
            traces_dir.clone(),
            config.max_file_size_mb,
            config.row_group_size,
        );

        Ok(Self { traces_dir, sqlite: Arc::new(sqlite), parquet_pool: Arc::new(parquet_pool) })
    }

    /// Get the traces directory path
    pub fn traces_dir(&self) -> &PathBuf {
        &self.traces_dir
    }

    /// Get the SQLite store
    pub fn sqlite(&self) -> &Arc<SqliteStore> {
        &self.sqlite
    }

    /// Get the Parquet writer pool
    pub fn parquet_pool(&self) -> &Arc<ParquetWriterPool> {
        &self.parquet_pool
    }
}
