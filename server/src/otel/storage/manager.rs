//! Trace storage manager (coordinates SQLite and Parquet)

use sqlx::SqlitePool;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::debug;

use super::parquet::ParquetWriterPool;
use super::sqlite::AttributeKeyCache;
use crate::core::config::OtelConfig;
use crate::otel::error::OtelError;

/// Trace storage manager
pub struct TraceStorageManager {
    traces_dir: PathBuf,
    pool: SqlitePool,
    parquet_pool: Arc<ParquetWriterPool>,
    attribute_cache: Arc<AttributeKeyCache>,
}

impl TraceStorageManager {
    /// Initialize the trace storage manager
    ///
    /// # Arguments
    /// * `data_dir` - The traces directory (e.g., from DataSubdir::Traces)
    /// * `config` - OTel configuration
    /// * `pool` - SQLite connection pool from DatabaseManager
    pub async fn init(
        data_dir: PathBuf,
        config: &OtelConfig,
        pool: SqlitePool,
    ) -> Result<Self, OtelError> {
        let traces_dir = data_dir.clone();
        tokio::fs::create_dir_all(&traces_dir)
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to create traces dir: {}", e)))?;

        debug!("Initializing trace storage at {:?}", traces_dir);

        // Cleanup orphan parquet files (from previous crashes)
        Self::cleanup_orphan_parquet_files(&traces_dir, &pool).await?;

        // Initialize and load attribute key cache
        let attribute_cache = Arc::new(AttributeKeyCache::new());
        attribute_cache.load_from_db(&pool).await?;
        debug!("Loaded attribute key cache from database");

        // Initialize Parquet writer pool
        let parquet_pool = ParquetWriterPool::new(
            traces_dir.clone(),
            config.storage.max_file_size_mb,
            config.storage.row_group_size,
        );

        Ok(Self { traces_dir, pool, parquet_pool: Arc::new(parquet_pool), attribute_cache })
    }

    /// Clean up orphan parquet files that exist on disk but aren't registered in SQLite.
    /// This handles crash recovery where parquet was written but SQLite transaction failed.
    async fn cleanup_orphan_parquet_files(
        traces_dir: &PathBuf,
        pool: &sqlx::SqlitePool,
    ) -> Result<(), OtelError> {
        use std::collections::HashSet;

        // Get all registered parquet files from SQLite
        let registered_files: HashSet<String> =
            sqlx::query_scalar::<_, String>("SELECT file_path FROM parquet_files")
                .fetch_all(pool)
                .await
                .map_err(|e| {
                    OtelError::StorageError(format!("Failed to get registered files: {}", e))
                })?
                .into_iter()
                .collect();

        // Scan all parquet files on disk
        let mut orphan_count = 0;
        let mut entries = tokio::fs::read_dir(traces_dir)
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to read traces dir: {}", e)))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to read dir entry: {}", e)))?
        {
            let path = entry.path();

            // Check subdirectories (date partitions like 2024-01-15/)
            if path.is_dir() {
                let mut sub_entries = tokio::fs::read_dir(&path).await.map_err(|e| {
                    OtelError::StorageError(format!("Failed to read subdir: {}", e))
                })?;

                while let Some(sub_entry) = sub_entries.next_entry().await.map_err(|e| {
                    OtelError::StorageError(format!("Failed to read subdir entry: {}", e))
                })? {
                    let file_path = sub_entry.path();
                    if file_path.extension().is_some_and(|ext| ext == "parquet") {
                        let file_path_str = file_path.to_string_lossy().to_string();
                        if !registered_files.contains(&file_path_str) {
                            debug!("Removing orphan parquet file: {:?}", file_path);
                            if let Err(e) = tokio::fs::remove_file(&file_path).await {
                                tracing::warn!(
                                    "Failed to remove orphan file {:?}: {}",
                                    file_path,
                                    e
                                );
                            } else {
                                orphan_count += 1;
                            }
                        }
                    }
                }
            }
        }

        if orphan_count > 0 {
            tracing::info!("Cleaned up {} orphan parquet files", orphan_count);
        }

        Ok(())
    }

    /// Get the traces directory path
    pub fn traces_dir(&self) -> &PathBuf {
        &self.traces_dir
    }

    /// Get the SQLite connection pool
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Get the Parquet writer pool
    pub fn parquet_pool(&self) -> &Arc<ParquetWriterPool> {
        &self.parquet_pool
    }

    /// Get the attribute key cache
    pub fn attribute_cache(&self) -> &Arc<AttributeKeyCache> {
        &self.attribute_cache
    }
}
