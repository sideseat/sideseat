//! Async parquet file reading with blocking I/O offloaded to thread pool

use arrow::array::RecordBatch;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::file::reader::{FileReader, SerializedFileReader};
use std::path::Path;

use crate::otel::error::OtelError;

/// Read all record batches from a parquet file
pub async fn read_parquet_file(path: &Path) -> Result<Vec<RecordBatch>, OtelError> {
    let path = path.to_path_buf();

    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&path)
            .map_err(|e| OtelError::StorageError(format!("Failed to open file: {}", e)))?;

        let builder = ParquetRecordBatchReaderBuilder::try_new(file)
            .map_err(|e| OtelError::StorageError(format!("Failed to create reader: {}", e)))?;

        let reader = builder
            .build()
            .map_err(|e| OtelError::StorageError(format!("Failed to build reader: {}", e)))?;

        let batches: Result<Vec<_>, _> = reader.collect();
        batches.map_err(|e| OtelError::StorageError(format!("Failed to read batches: {}", e)))
    })
    .await
    .map_err(|e| OtelError::StorageError(format!("Task join error: {}", e)))?
}

/// Get parquet file metadata
pub async fn get_parquet_metadata(path: &Path) -> Result<ParquetMetadata, OtelError> {
    let path = path.to_path_buf();

    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&path)
            .map_err(|e| OtelError::StorageError(format!("Failed to open file: {}", e)))?;

        let reader = SerializedFileReader::new(file)
            .map_err(|e| OtelError::StorageError(format!("Failed to read metadata: {}", e)))?;

        let file_metadata = reader.metadata().file_metadata();

        Ok(ParquetMetadata {
            num_rows: file_metadata.num_rows() as u64,
            num_row_groups: reader.num_row_groups() as u32,
            created_by: file_metadata.created_by().map(|s| s.to_string()),
        })
    })
    .await
    .map_err(|e| OtelError::StorageError(format!("Task join error: {}", e)))?
}

/// Parquet file metadata
#[derive(Debug)]
pub struct ParquetMetadata {
    pub num_rows: u64,
    pub num_row_groups: u32,
    pub created_by: Option<String>,
}
