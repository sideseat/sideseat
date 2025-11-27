//! Thread-safe pool of date-partitioned parquet writers

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::writer::{DayWriter, WrittenFile};
use crate::otel::error::OtelError;
use crate::otel::normalize::NormalizedSpan;

/// Manages DayWriter instances keyed by date string (YYYY-MM-DD)
pub struct ParquetWriterPool {
    traces_dir: PathBuf,
    max_file_size_mb: u32,
    row_group_size: usize,
    writers: RwLock<HashMap<String, Arc<DayWriter>>>,
}

impl ParquetWriterPool {
    pub fn new(traces_dir: PathBuf, max_file_size_mb: u32, row_group_size: usize) -> Self {
        Self { traces_dir, max_file_size_mb, row_group_size, writers: RwLock::new(HashMap::new()) }
    }

    /// Get existing writer or create new one for the given date
    pub async fn get_or_create(&self, date: &str) -> Arc<DayWriter> {
        {
            let readers = self.writers.read().await;
            if let Some(writer) = readers.get(date) {
                return writer.clone();
            }
        }

        let mut writers = self.writers.write().await;

        // Double-check after acquiring write lock
        if let Some(writer) = writers.get(date) {
            return writer.clone();
        }

        let writer = Arc::new(DayWriter::new(
            date,
            self.traces_dir.clone(),
            self.max_file_size_mb,
            self.row_group_size,
        ));

        writers.insert(date.to_string(), writer.clone());
        writer
    }

    /// Route spans to appropriate day writers and write to parquet files
    pub async fn write_batch(
        &self,
        spans: Vec<NormalizedSpan>,
    ) -> Result<Vec<WrittenFile>, OtelError> {
        if spans.is_empty() {
            return Ok(vec![]);
        }

        let mut by_date: HashMap<String, Vec<NormalizedSpan>> = HashMap::new();
        for span in spans {
            let date = timestamp_to_date(span.start_time_unix_nano);
            by_date.entry(date).or_default().push(span);
        }

        let mut written_files = Vec::new();
        for (date, date_spans) in by_date {
            let writer = self.get_or_create(&date).await;
            if let Some(file) = writer.write_batch(&date_spans).await? {
                written_files.push(file);
            }
        }

        Ok(written_files)
    }

    /// Flush all active writers to disk
    pub async fn flush_all(&self) -> Result<Vec<WrittenFile>, OtelError> {
        let writers = self.writers.read().await;
        let mut written_files = Vec::new();

        for writer in writers.values() {
            if let Some(file) = writer.flush().await? {
                written_files.push(file);
            }
        }

        Ok(written_files)
    }

    /// Remove in-memory writers for dates older than keep_days
    pub async fn cleanup_old_writers(&self, keep_days: usize) {
        let cutoff = chrono::Utc::now()
            .checked_sub_signed(chrono::Duration::days(keep_days as i64))
            .map(|d| d.format("%Y-%m-%d").to_string());

        if let Some(cutoff_date) = cutoff {
            let mut writers = self.writers.write().await;
            writers.retain(|date, _| date >= &cutoff_date);
        }
    }
}

/// Convert nanosecond timestamp to date string
fn timestamp_to_date(ns: i64) -> String {
    use chrono::{DateTime, Utc};
    let secs = ns / 1_000_000_000;
    let nsecs = (ns % 1_000_000_000) as u32;
    DateTime::from_timestamp(secs, nsecs).unwrap_or_else(Utc::now).format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timestamp_to_date_epoch() {
        // Unix epoch: 1970-01-01
        let date = timestamp_to_date(0);
        assert_eq!(date, "1970-01-01");
    }

    #[test]
    fn test_timestamp_to_date_known_date() {
        // 2024-01-15 00:00:00 UTC = 1705276800 seconds
        let ns = 1705276800_i64 * 1_000_000_000;
        let date = timestamp_to_date(ns);
        assert_eq!(date, "2024-01-15");
    }

    #[test]
    fn test_timestamp_to_date_with_nanoseconds() {
        // Same date with nanoseconds shouldn't change the date
        let ns = 1705276800_i64 * 1_000_000_000 + 500_000_000; // +0.5 seconds
        let date = timestamp_to_date(ns);
        assert_eq!(date, "2024-01-15");
    }

    #[test]
    fn test_timestamp_to_date_end_of_day() {
        // 2024-01-15 23:59:59.999999999 UTC
        let ns = (1705276800_i64 + 86399) * 1_000_000_000 + 999_999_999;
        let date = timestamp_to_date(ns);
        assert_eq!(date, "2024-01-15");
    }

    #[test]
    fn test_parquet_writer_pool_new() {
        let pool = ParquetWriterPool::new(PathBuf::from("/tmp/traces"), 64, 10000);
        assert_eq!(pool.max_file_size_mb, 64);
        assert_eq!(pool.row_group_size, 10000);
    }

    #[tokio::test]
    async fn test_parquet_writer_pool_get_or_create() {
        let pool = ParquetWriterPool::new(PathBuf::from("/tmp/traces"), 64, 10000);

        // First call creates a writer
        let writer1 = pool.get_or_create("2024-01-15").await;

        // Second call returns the same writer
        let writer2 = pool.get_or_create("2024-01-15").await;
        assert!(Arc::ptr_eq(&writer1, &writer2));

        // Different date creates different writer
        let writer3 = pool.get_or_create("2024-01-16").await;
        assert!(!Arc::ptr_eq(&writer1, &writer3));
    }

    #[tokio::test]
    async fn test_parquet_writer_pool_cleanup_old_writers() {
        let pool = ParquetWriterPool::new(PathBuf::from("/tmp/traces"), 64, 10000);

        // Create writers for different dates
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let old_date = "2020-01-01"; // Very old date

        pool.get_or_create(&today).await;
        pool.get_or_create(old_date).await;

        // Before cleanup: 2 writers
        {
            let writers = pool.writers.read().await;
            assert_eq!(writers.len(), 2);
        }

        // Cleanup keeping only 7 days
        pool.cleanup_old_writers(7).await;

        // After cleanup: only today's writer remains
        {
            let writers = pool.writers.read().await;
            assert_eq!(writers.len(), 1);
            assert!(writers.contains_key(&today));
            assert!(!writers.contains_key(old_date));
        }
    }

    #[tokio::test]
    async fn test_parquet_writer_pool_write_batch_empty() {
        let pool = ParquetWriterPool::new(PathBuf::from("/tmp/traces"), 64, 10000);
        let result = pool.write_batch(vec![]).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }
}
