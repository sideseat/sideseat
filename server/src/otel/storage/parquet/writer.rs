//! Parquet file writer for span data

use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;
use std::path::PathBuf;
use tokio::sync::Mutex;
use tracing::debug;

use crate::otel::error::OtelError;
use crate::otel::normalize::NormalizedSpan;
use crate::otel::schema::span::{SpanSchema, to_record_batch};

/// Writer for a single day's parquet files
pub struct DayWriter {
    date: String,
    traces_dir: PathBuf,
    current_file: Mutex<Option<ActiveFile>>,
    max_file_size: u64,
    row_group_size: usize,
    part_counter: Mutex<u32>,
}

struct ActiveFile {
    path: PathBuf,
    writer: ArrowWriter<std::fs::File>,
    current_size: u64,
    span_count: u32,
    min_start_time: i64,
    max_end_time: i64,
}

impl DayWriter {
    /// Create a new day writer
    pub fn new(
        date: &str,
        traces_dir: PathBuf,
        max_file_size_mb: u32,
        row_group_size: usize,
    ) -> Self {
        Self {
            date: date.to_string(),
            traces_dir,
            current_file: Mutex::new(None),
            max_file_size: (max_file_size_mb as u64) * 1024 * 1024,
            row_group_size,
            part_counter: Mutex::new(0),
        }
    }

    /// Write spans to the current parquet file, rotating if size limit exceeded
    pub async fn write_batch(
        &self,
        spans: &[NormalizedSpan],
    ) -> Result<Option<WrittenFile>, OtelError> {
        if spans.is_empty() {
            return Ok(None);
        }

        let record_batch = to_record_batch(spans)?;
        let batch_memory_size = record_batch.get_array_memory_size() as u64;

        let mut file_guard = self.current_file.lock().await;

        if file_guard.is_none() {
            *file_guard = Some(self.create_new_file().await?);
        }

        let mut active = file_guard
            .take()
            .ok_or_else(|| OtelError::StorageError("File initialization failed".to_string()))?;

        active.span_count += spans.len() as u32;
        for span in spans {
            active.min_start_time = active.min_start_time.min(span.start_time_unix_nano);
            if let Some(end) = span.end_time_unix_nano {
                active.max_end_time = active.max_end_time.max(end);
            }
        }
        active.current_size += batch_memory_size;

        let result =
            tokio::task::spawn_blocking(move || {
                active.writer.write(&record_batch).map_err(|e| {
                    OtelError::StorageError(format!("Failed to write batch: {}", e))
                })?;

                active.writer.flush().map_err(|e| {
                    OtelError::StorageError(format!("Failed to flush batch: {}", e))
                })?;

                Ok::<_, OtelError>(active)
            })
            .await
            .map_err(|e| OtelError::StorageError(format!("Task join error: {}", e)))??;

        let should_rotate = result.current_size >= self.max_file_size;
        *file_guard = Some(result);

        if should_rotate {
            return self.close_current_file(&mut file_guard).await;
        }

        Ok(None)
    }

    /// Close and flush the current file to disk
    pub async fn flush(&self) -> Result<Option<WrittenFile>, OtelError> {
        let mut file = self.current_file.lock().await;
        if file.is_some() { self.close_current_file(&mut file).await } else { Ok(None) }
    }

    /// Validate date format (YYYY-MM-DD) and return (year, month, day)
    fn validate_date(&self) -> Result<(&str, &str, &str), OtelError> {
        let parts: Vec<&str> = self.date.split('-').collect();
        let invalid_date = || OtelError::StorageError("Invalid date partition".to_string());

        if parts.len() != 3 {
            tracing::warn!("Invalid date format: {}", self.date);
            return Err(invalid_date());
        }

        let year = parts[0];
        let month = parts[1];
        let day = parts[2];

        // Validate format: YYYY-MM-DD with all digits
        if year.len() != 4 || !year.chars().all(|c| c.is_ascii_digit()) {
            tracing::warn!("Invalid year format in date: {}", self.date);
            return Err(invalid_date());
        }
        if month.len() != 2 || !month.chars().all(|c| c.is_ascii_digit()) {
            tracing::warn!("Invalid month format in date: {}", self.date);
            return Err(invalid_date());
        }
        if day.len() != 2 || !day.chars().all(|c| c.is_ascii_digit()) {
            tracing::warn!("Invalid day format in date: {}", self.date);
            return Err(invalid_date());
        }

        // Validate numeric ranges - use map_err to handle parse failures
        let year_num: u32 = year.parse().map_err(|_| {
            tracing::warn!("Failed to parse year: {}", year);
            invalid_date()
        })?;
        let month_num: u32 = month.parse().map_err(|_| {
            tracing::warn!("Failed to parse month: {}", month);
            invalid_date()
        })?;
        let day_num: u32 = day.parse().map_err(|_| {
            tracing::warn!("Failed to parse day: {}", day);
            invalid_date()
        })?;

        if !(2000..=2100).contains(&year_num) {
            tracing::warn!("Year out of range: {}", year_num);
            return Err(invalid_date());
        }
        if !(1..=12).contains(&month_num) {
            tracing::warn!("Month out of range: {}", month_num);
            return Err(invalid_date());
        }
        if !(1..=31).contains(&day_num) {
            tracing::warn!("Day out of range: {}", day_num);
            return Err(invalid_date());
        }

        Ok((year, month, day))
    }

    /// Create a new parquet file in the Hive-partitioned directory structure
    async fn create_new_file(&self) -> Result<ActiveFile, OtelError> {
        let (year, month, day) = self.validate_date()?;

        let dir = self
            .traces_dir
            .join(format!("yyyy={}", year))
            .join(format!("mm={}", month))
            .join(format!("dd={}", day));

        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to create dir: {}", e)))?;

        let mut counter = self.part_counter.lock().await;
        *counter += 1;
        let filename = format!("spans-{}-part{:04}.parquet", self.date, *counter);
        let path = dir.join(&filename);

        let row_group_size = self.row_group_size;
        let path_clone = path.clone();
        let writer = tokio::task::spawn_blocking(move || {
            let file = std::fs::File::create(&path_clone)
                .map_err(|e| OtelError::StorageError(format!("Failed to create file: {}", e)))?;

            let props = WriterProperties::builder()
                .set_compression(Compression::ZSTD(ZstdLevel::try_new(3).unwrap_or_default()))
                .set_max_row_group_size(row_group_size)
                .build();

            let schema = SpanSchema::arrow_schema();
            ArrowWriter::try_new(file, schema, Some(props))
                .map_err(|e| OtelError::StorageError(format!("Failed to create writer: {}", e)))
        })
        .await
        .map_err(|e| OtelError::StorageError(format!("Task join error: {}", e)))??;

        Ok(ActiveFile {
            path,
            writer,
            current_size: 0,
            span_count: 0,
            min_start_time: i64::MAX,
            max_end_time: 0,
        })
    }

    /// Close the current file, sync to disk, and return file metadata
    async fn close_current_file(
        &self,
        file: &mut Option<ActiveFile>,
    ) -> Result<Option<WrittenFile>, OtelError> {
        if let Some(active) = file.take() {
            let path = active.path.clone();
            let span_count = active.span_count;
            let min_start_time = active.min_start_time;
            let max_end_time = active.max_end_time;
            let date_partition = self.date.clone();

            let file_size = tokio::task::spawn_blocking(move || {
                // Write parquet footer and close
                active
                    .writer
                    .close()
                    .map_err(|e| OtelError::StorageError(format!("Failed to close file: {}", e)))?;

                // Ensure durability on process termination
                if let Ok(file) = std::fs::File::open(&path) {
                    let _ = file.sync_all();
                }

                let metadata = std::fs::metadata(&path).map_err(|e| {
                    OtelError::StorageError(format!("Failed to get metadata: {}", e))
                })?;

                Ok::<_, OtelError>((path, metadata.len()))
            })
            .await
            .map_err(|e| OtelError::StorageError(format!("Task join error: {}", e)))??;

            let (path, size) = file_size;

            debug!("Closed parquet file: {:?} ({} spans, {} bytes)", path, span_count, size);

            return Ok(Some(WrittenFile {
                path,
                date_partition,
                span_count,
                file_size: size,
                min_start_time,
                max_end_time,
            }));
        }
        Ok(None)
    }
}

/// Information about a written parquet file
#[derive(Debug)]
pub struct WrittenFile {
    pub path: PathBuf,
    pub date_partition: String,
    pub span_count: u32,
    pub file_size: u64,
    pub min_start_time: i64,
    pub max_end_time: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn create_writer_with_date(date: &str) -> DayWriter {
        DayWriter::new(date, PathBuf::from("/tmp/test"), 64, 10000)
    }

    #[test]
    fn test_validate_date_valid() {
        let writer = create_writer_with_date("2024-01-15");
        let result = writer.validate_date();
        assert!(result.is_ok());
        let (year, month, day) = result.unwrap();
        assert_eq!(year, "2024");
        assert_eq!(month, "01");
        assert_eq!(day, "15");
    }

    #[test]
    fn test_validate_date_boundary_values() {
        // Test min year
        let writer = create_writer_with_date("2000-01-01");
        assert!(writer.validate_date().is_ok());

        // Test max year
        let writer = create_writer_with_date("2100-12-31");
        assert!(writer.validate_date().is_ok());
    }

    #[test]
    fn test_validate_date_invalid_format_no_dashes() {
        let writer = create_writer_with_date("20240115");
        assert!(writer.validate_date().is_err());
    }

    #[test]
    fn test_validate_date_invalid_format_wrong_separators() {
        let writer = create_writer_with_date("2024/01/15");
        assert!(writer.validate_date().is_err());
    }

    #[test]
    fn test_validate_date_invalid_year_too_short() {
        let writer = create_writer_with_date("24-01-15");
        assert!(writer.validate_date().is_err());
    }

    #[test]
    fn test_validate_date_invalid_year_too_long() {
        let writer = create_writer_with_date("12024-01-15");
        assert!(writer.validate_date().is_err());
    }

    #[test]
    fn test_validate_date_invalid_month_single_digit() {
        let writer = create_writer_with_date("2024-1-15");
        assert!(writer.validate_date().is_err());
    }

    #[test]
    fn test_validate_date_invalid_day_single_digit() {
        let writer = create_writer_with_date("2024-01-5");
        assert!(writer.validate_date().is_err());
    }

    #[test]
    fn test_validate_date_year_out_of_range_low() {
        let writer = create_writer_with_date("1999-01-15");
        assert!(writer.validate_date().is_err());
    }

    #[test]
    fn test_validate_date_year_out_of_range_high() {
        let writer = create_writer_with_date("2101-01-15");
        assert!(writer.validate_date().is_err());
    }

    #[test]
    fn test_validate_date_month_out_of_range() {
        let writer = create_writer_with_date("2024-13-15");
        assert!(writer.validate_date().is_err());

        let writer = create_writer_with_date("2024-00-15");
        assert!(writer.validate_date().is_err());
    }

    #[test]
    fn test_validate_date_day_out_of_range() {
        let writer = create_writer_with_date("2024-01-32");
        assert!(writer.validate_date().is_err());

        let writer = create_writer_with_date("2024-01-00");
        assert!(writer.validate_date().is_err());
    }

    #[test]
    fn test_validate_date_non_numeric_year() {
        let writer = create_writer_with_date("20a4-01-15");
        assert!(writer.validate_date().is_err());
    }

    #[test]
    fn test_validate_date_non_numeric_month() {
        let writer = create_writer_with_date("2024-0a-15");
        assert!(writer.validate_date().is_err());
    }

    #[test]
    fn test_validate_date_non_numeric_day() {
        let writer = create_writer_with_date("2024-01-1a");
        assert!(writer.validate_date().is_err());
    }

    #[test]
    fn test_validate_date_path_traversal_attempt() {
        // Path traversal attempts should fail validation
        let writer = create_writer_with_date("../../../etc/passwd");
        assert!(writer.validate_date().is_err());

        let writer = create_writer_with_date("2024-../-01");
        assert!(writer.validate_date().is_err());
    }

    #[test]
    fn test_validate_date_empty_string() {
        let writer = create_writer_with_date("");
        assert!(writer.validate_date().is_err());
    }

    #[test]
    fn test_validate_date_extra_parts() {
        let writer = create_writer_with_date("2024-01-15-extra");
        assert!(writer.validate_date().is_err());
    }

    #[test]
    fn test_day_writer_new() {
        let writer = DayWriter::new("2024-01-15", PathBuf::from("/tmp"), 64, 10000);
        assert_eq!(writer.date, "2024-01-15");
        assert_eq!(writer.max_file_size, 64 * 1024 * 1024);
        assert_eq!(writer.row_group_size, 10000);
    }
}
