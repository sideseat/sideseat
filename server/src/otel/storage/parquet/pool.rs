//! Pool of day writers for concurrent writes

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::writer::{DayWriter, WrittenFile};
use crate::otel::error::OtelError;
use crate::otel::normalize::NormalizedSpan;

/// Pool of day writers with thread-safe access
pub struct ParquetWriterPool {
    traces_dir: PathBuf,
    max_file_size_mb: u32,
    row_group_size: usize,
    writers: RwLock<HashMap<String, Arc<DayWriter>>>,
}

impl ParquetWriterPool {
    /// Create a new writer pool
    pub fn new(traces_dir: PathBuf, max_file_size_mb: u32, row_group_size: usize) -> Self {
        Self { traces_dir, max_file_size_mb, row_group_size, writers: RwLock::new(HashMap::new()) }
    }

    /// Get or create a writer for a specific date
    pub async fn get_or_create(&self, date: &str) -> Arc<DayWriter> {
        // Try read lock first
        {
            let readers = self.writers.read().await;
            if let Some(writer) = readers.get(date) {
                return writer.clone();
            }
        }

        // Need write lock to create
        let mut writers = self.writers.write().await;

        // Double-check (another task might have created it)
        if let Some(writer) = writers.get(date) {
            return writer.clone();
        }

        // Create new writer
        let writer = Arc::new(DayWriter::new(
            date,
            self.traces_dir.clone(),
            self.max_file_size_mb,
            self.row_group_size,
        ));

        writers.insert(date.to_string(), writer.clone());
        writer
    }

    /// Write a batch of spans (automatically routes to correct day writer)
    pub async fn write_batch(
        &self,
        spans: &[NormalizedSpan],
    ) -> Result<Vec<WrittenFile>, OtelError> {
        if spans.is_empty() {
            return Ok(vec![]);
        }

        // Group spans by date
        let mut by_date: HashMap<String, Vec<&NormalizedSpan>> = HashMap::new();
        for span in spans {
            let date = timestamp_to_date(span.start_time_unix_nano);
            by_date.entry(date).or_default().push(span);
        }

        // Write each group
        let mut written_files = Vec::new();
        for (date, date_spans) in by_date {
            let writer = self.get_or_create(&date).await;
            let owned_spans: Vec<NormalizedSpan> = date_spans.into_iter().cloned().collect();
            if let Some(file) = writer.write_batch(&owned_spans).await? {
                written_files.push(file);
            }
        }

        Ok(written_files)
    }

    /// Flush all writers
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

    /// Remove old writers (for dates no longer active)
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
