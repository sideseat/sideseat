//! DuckDB retention management
//!
//! Efficient batch deletion with transaction-safe cascading deletes.
//! Note: DuckDB doesn't support data-modifying CTEs, so we use explicit transactions.

use std::collections::HashMap;

use chrono::{TimeDelta, Utc};
use duckdb::Connection;

use super::{DuckdbError, in_transaction};
use crate::core::config::RetentionConfig;

/// Result of retention cleanup, including trace IDs for file cleanup
#[derive(Default)]
pub struct RetentionResult {
    /// Total spans deleted
    pub deleted_count: u64,
    /// Trace IDs grouped by project for file cleanup
    pub trace_ids_by_project: HashMap<String, Vec<String>>,
}

/// Run retention cleanup based on config
/// Returns trace IDs for file cleanup. Runs CHECKPOINT after deletions to reclaim space.
pub fn run_retention(
    conn: &Connection,
    config: &RetentionConfig,
) -> Result<RetentionResult, DuckdbError> {
    let mut result = RetentionResult::default();

    if let Some(max_age_minutes) = config.max_age_minutes {
        // Span cleanup
        let (deleted, trace_ids) = cleanup_by_time(conn, max_age_minutes)?;
        if deleted > 0 {
            tracing::debug!(
                deleted,
                max_age_minutes,
                "Time-based span retention cleanup"
            );
            result.deleted_count += deleted;
            merge_trace_ids(&mut result.trace_ids_by_project, trace_ids);
        }

        // Metrics cleanup (same time threshold)
        let deleted = cleanup_metrics_by_time(conn, max_age_minutes)?;
        if deleted > 0 {
            tracing::debug!(
                deleted,
                max_age_minutes,
                "Time-based metrics retention cleanup"
            );
            result.deleted_count += deleted;
        }
    }

    if let Some(max_spans) = config.max_spans {
        let (deleted, trace_ids) = cleanup_by_count(conn, max_spans)?;
        if deleted > 0 {
            tracing::debug!(deleted, max_spans, "Count-based retention cleanup");
            result.deleted_count += deleted;
            merge_trace_ids(&mut result.trace_ids_by_project, trace_ids);
        }
    }

    if result.deleted_count > 0 {
        // CHECKPOINT ensures deleted data is flushed
        // Note: DuckDB doesn't shrink the file - freed space is reused internally
        conn.execute("CHECKPOINT", [])?;
        tracing::debug!(
            deleted = result.deleted_count,
            projects = result.trace_ids_by_project.len(),
            "Retention cleanup completed, checkpoint done"
        );
    } else {
        tracing::debug!("Retention check complete, nothing to delete");
    }

    Ok(result)
}

/// Merge trace IDs from a batch into the cumulative result
fn merge_trace_ids(
    target: &mut HashMap<String, Vec<String>>,
    source: HashMap<String, Vec<String>>,
) {
    for (project_id, trace_ids) in source {
        target.entry(project_id).or_default().extend(trace_ids);
    }
}

/// Max spans to delete per batch (prevents long transactions)
const RETENTION_BATCH_SIZE: i64 = 100_000;

/// Max batches per time-based cleanup cycle (prevents unbounded blocking)
const MAX_TIME_CLEANUP_BATCHES: usize = 10;

/// Max trace IDs to collect per cleanup cycle (prevents memory explosion)
const MAX_TRACE_IDS_PER_CYCLE: usize = 10_000;

/// Execute retention based on time limit (delete spans older than N minutes)
/// Iterates in batches with a limit to prevent unbounded blocking
/// Returns (spans_deleted, trace_ids_by_project) for file cleanup
pub fn cleanup_by_time(
    conn: &Connection,
    minutes: u64,
) -> Result<(u64, HashMap<String, Vec<String>>), DuckdbError> {
    let minutes_i64 = i64::try_from(minutes).unwrap_or(i64::MAX);
    let cutoff = Utc::now() - TimeDelta::minutes(minutes_i64);
    let cutoff_str = cutoff.format("%Y-%m-%d %H:%M:%S%.6f").to_string();
    tracing::debug!(%cutoff_str, minutes, "Time-based retention check");

    let mut total_deleted = 0u64;
    let mut all_trace_ids: HashMap<String, Vec<String>> = HashMap::new();

    for _ in 0..MAX_TIME_CLEANUP_BATCHES {
        let (deleted, trace_ids) = delete_spans_before(conn, &cutoff_str, RETENTION_BATCH_SIZE)?;
        if deleted == 0 {
            break;
        }
        tracing::debug!(deleted, "Deleted batch of expired spans");
        total_deleted += deleted;
        merge_trace_ids(&mut all_trace_ids, trace_ids);
    }
    Ok((total_deleted, all_trace_ids))
}

/// Max batches per count-based cleanup cycle (prevents unbounded blocking)
const MAX_COUNT_CLEANUP_BATCHES: usize = 10;

/// Execute retention based on max span count (delete oldest spans exceeding limit)
/// Returns (spans_deleted, trace_ids_by_project). Iterates in batches with a limit to prevent unbounded blocking.
pub fn cleanup_by_count(
    conn: &Connection,
    max_spans: u64,
) -> Result<(u64, HashMap<String, Vec<String>>), DuckdbError> {
    // Use COUNT(span_id) to leverage primary key index for faster counting
    let span_count: i64 = conn
        .query_row("SELECT COUNT(span_id) FROM otel_spans", [], |row| {
            row.get(0)
        })
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "Failed to query span count");
            0
        });

    let max_spans_i64 = i64::try_from(max_spans).unwrap_or(i64::MAX);
    if span_count <= max_spans_i64 {
        tracing::debug!(span_count, max_spans, "Span count within limit");
        return Ok((0, HashMap::new()));
    }

    let to_delete = span_count - max_spans_i64;
    tracing::debug!(
        span_count,
        max_spans,
        to_delete,
        "Span count exceeds limit, cleaning up"
    );

    // Delete in batches to avoid long transactions, with a limit to prevent unbounded blocking
    let mut total_deleted = 0u64;
    let mut all_trace_ids: HashMap<String, Vec<String>> = HashMap::new();
    let mut remaining = to_delete;
    for _ in 0..MAX_COUNT_CLEANUP_BATCHES {
        if remaining <= 0 {
            break;
        }
        let batch = remaining.min(RETENTION_BATCH_SIZE);
        let (deleted, trace_ids) = delete_oldest_spans(conn, batch)?;
        if deleted == 0 {
            break;
        }
        tracing::debug!(deleted, "Deleted batch of excess spans");
        total_deleted += deleted;
        merge_trace_ids(&mut all_trace_ids, trace_ids);
        remaining = remaining.saturating_sub(deleted as i64);
    }

    Ok((total_deleted, all_trace_ids))
}

/// Delete spans before cutoff timestamp (for time-based retention)
/// Returns (deleted_count, trace_ids_by_project)
fn delete_spans_before(
    conn: &Connection,
    cutoff: &str,
    limit: i64,
) -> Result<(u64, HashMap<String, Vec<String>>), DuckdbError> {
    delete_spans_with_query(
        conn,
        "INSERT INTO _retention_batch
         SELECT trace_id, span_id FROM otel_spans
         WHERE timestamp_start < ?1
         ORDER BY timestamp_start ASC
         LIMIT ?2",
        &[&cutoff as &dyn duckdb::ToSql, &limit],
    )
}

/// Delete oldest N spans (for count-based retention)
/// Returns (deleted_count, trace_ids_by_project)
fn delete_oldest_spans(
    conn: &Connection,
    limit: i64,
) -> Result<(u64, HashMap<String, Vec<String>>), DuckdbError> {
    delete_spans_with_query(
        conn,
        "INSERT INTO _retention_batch
         SELECT trace_id, span_id FROM otel_spans
         ORDER BY timestamp_start ASC
         LIMIT ?1",
        &[&limit as &dyn duckdb::ToSql],
    )
}

/// Common delete logic using temp table for efficiency
/// Returns (deleted_count, trace_ids_by_project) for file cleanup
fn delete_spans_with_query(
    conn: &Connection,
    insert_sql: &str,
    params: &[&dyn duckdb::ToSql],
) -> Result<(u64, HashMap<String, Vec<String>>), DuckdbError> {
    in_transaction(conn, |conn| {
        // Create/clear temp table (query runs once, not 2x)
        conn.execute(
            "CREATE TEMP TABLE IF NOT EXISTS _retention_batch (
                trace_id VARCHAR NOT NULL,
                span_id VARCHAR NOT NULL,
                PRIMARY KEY (trace_id, span_id)
            )",
            [],
        )?;
        conn.execute("DELETE FROM _retention_batch", [])?;

        // Populate with spans to delete
        conn.execute(insert_sql, params)?;

        // Collect distinct (project_id, trace_id) pairs BEFORE deletion for file cleanup
        // _retention_batch only has (trace_id, span_id), need JOIN with otel_spans for project_id
        // LIMIT prevents memory explosion for large cleanup batches
        let trace_ids_by_project = collect_trace_ids_for_cleanup(conn)?;

        // Delete spans (events, links, and messages are embedded in span rows)
        let deleted = conn.execute(
            "DELETE FROM otel_spans
             WHERE (trace_id, span_id) IN (SELECT trace_id, span_id FROM _retention_batch)",
            [],
        )?;

        Ok((deleted as u64, trace_ids_by_project))
    })
}

/// Collect distinct (project_id, trace_id) pairs from the retention batch for file cleanup
fn collect_trace_ids_for_cleanup(
    conn: &Connection,
) -> Result<HashMap<String, Vec<String>>, DuckdbError> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT os.project_id, os.trace_id
         FROM _retention_batch rb
         JOIN otel_spans os ON rb.trace_id = os.trace_id AND rb.span_id = os.span_id
         LIMIT ?1",
    )?;

    let limit = MAX_TRACE_IDS_PER_CYCLE as i64;
    let rows = stmt.query_map([limit], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    let mut result: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        let (project_id, trace_id) = row?;
        result.entry(project_id).or_default().push(trace_id);
    }

    Ok(result)
}

// ============================================================================
// METRICS RETENTION
// ============================================================================

/// Max batches per metrics cleanup cycle (prevents unbounded blocking)
const MAX_METRICS_CLEANUP_BATCHES: usize = 10;

/// Execute retention based on time limit (delete metrics older than N minutes)
/// Iterates in batches with a limit to prevent unbounded blocking.
pub fn cleanup_metrics_by_time(conn: &Connection, minutes: u64) -> Result<u64, DuckdbError> {
    let minutes_i64 = i64::try_from(minutes).unwrap_or(i64::MAX);
    let cutoff = Utc::now() - TimeDelta::minutes(minutes_i64);
    let cutoff_str = cutoff.format("%Y-%m-%d %H:%M:%S%.6f").to_string();
    tracing::debug!(%cutoff_str, minutes, "Time-based metrics retention check");

    let mut total_deleted = 0u64;
    for _ in 0..MAX_METRICS_CLEANUP_BATCHES {
        let deleted = delete_metrics_before(conn, &cutoff_str, RETENTION_BATCH_SIZE)?;
        if deleted == 0 {
            break;
        }
        tracing::debug!(deleted, "Deleted batch of expired metrics");
        total_deleted += deleted;
    }
    Ok(total_deleted)
}

/// Delete metrics before cutoff timestamp (for time-based retention)
fn delete_metrics_before(conn: &Connection, cutoff: &str, limit: i64) -> Result<u64, DuckdbError> {
    let deleted = conn.execute(
        "DELETE FROM otel_metrics
         WHERE rowid IN (
             SELECT rowid FROM otel_metrics
             WHERE timestamp < ?1
             ORDER BY timestamp ASC
             LIMIT ?2
         )",
        duckdb::params![cutoff, limit],
    )?;
    Ok(deleted as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::storage::AppStorage;
    use crate::data::duckdb::DuckdbService;
    use chrono::Utc;
    use tempfile::TempDir;

    async fn create_test_service() -> (TempDir, DuckdbService) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let duckdb_dir = temp_dir.path().join("duckdb");
        tokio::fs::create_dir_all(&duckdb_dir)
            .await
            .expect("Failed to create duckdb dir");
        let storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
        let service = DuckdbService::init(&storage)
            .await
            .expect("Failed to init analytics service");
        (temp_dir, service)
    }

    fn insert_test_span(conn: &Connection, trace_id: &str, span_id: &str, timestamp: &str) {
        conn.execute(
            "INSERT INTO otel_spans (trace_id, span_id, span_name, timestamp_start, project_id)
             VALUES (?1, ?2, 'test', ?3, 'default')",
            [trace_id, span_id, timestamp],
        )
        .expect("Failed to insert test span");
    }

    #[tokio::test]
    async fn test_cleanup_by_time_empty_table() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        let (deleted, trace_ids) = cleanup_by_time(&conn, 60).expect("Should cleanup");
        assert_eq!(deleted, 0);
        assert!(trace_ids.is_empty());
    }

    #[tokio::test]
    async fn test_cleanup_by_time_removes_old_spans() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Insert spans: 2 old, 1 recent
        insert_test_span(&conn, "trace1", "span1", "2020-01-01 00:00:00");
        insert_test_span(&conn, "trace2", "span2", "2020-01-02 00:00:00");
        let recent = Utc::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string();
        insert_test_span(&conn, "trace3", "span3", &recent);

        // Cleanup spans older than 1 minute
        let (deleted, _trace_ids) = cleanup_by_time(&conn, 1).expect("Should cleanup");
        assert_eq!(deleted, 2);

        // Verify only recent span remains
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
            .expect("Should query");
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_cleanup_by_count_empty_table() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        let (deleted, trace_ids) = cleanup_by_count(&conn, 100).expect("Should cleanup");
        assert_eq!(deleted, 0);
        assert!(trace_ids.is_empty());
    }

    #[tokio::test]
    async fn test_cleanup_by_count_under_limit() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Insert a few spans (under the limit)
        insert_test_span(&conn, "trace1", "span1", "2020-01-01 00:00:00");
        insert_test_span(&conn, "trace2", "span2", "2020-01-02 00:00:00");

        // 100 span limit - should not delete anything (only 2 spans)
        let (deleted, trace_ids) = cleanup_by_count(&conn, 100).expect("Should cleanup");
        assert_eq!(deleted, 0);
        assert!(trace_ids.is_empty());

        // Verify spans still exist
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
            .expect("Should query");
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_cleanup_by_count_exceeds_limit() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Insert 5 spans
        insert_test_span(&conn, "trace1", "span1", "2020-01-01 00:00:00");
        insert_test_span(&conn, "trace2", "span2", "2020-01-02 00:00:00");
        insert_test_span(&conn, "trace3", "span3", "2020-01-03 00:00:00");
        insert_test_span(&conn, "trace4", "span4", "2020-01-04 00:00:00");
        insert_test_span(&conn, "trace5", "span5", "2020-01-05 00:00:00");

        // Limit to 2 spans - should delete 3 oldest
        let (deleted, _trace_ids) = cleanup_by_count(&conn, 2).expect("Should cleanup");
        assert_eq!(deleted, 3);

        // Verify only 2 spans remain (the newest ones)
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
            .expect("Should query");
        assert_eq!(count, 2);

        // Verify the newest spans were kept
        let trace4_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM otel_spans WHERE trace_id = 'trace4'",
                [],
                |row| row.get(0),
            )
            .expect("Should query");
        assert_eq!(trace4_exists, 1);

        let trace5_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM otel_spans WHERE trace_id = 'trace5'",
                [],
                |row| row.get(0),
            )
            .expect("Should query");
        assert_eq!(trace5_exists, 1);
    }

    #[tokio::test]
    async fn test_cleanup_preserves_recent_spans() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Insert only recent spans
        let now = Utc::now();
        let recent1 = now.format("%Y-%m-%d %H:%M:%S%.6f").to_string();
        let recent2 = (now - TimeDelta::seconds(30))
            .format("%Y-%m-%d %H:%M:%S%.6f")
            .to_string();

        insert_test_span(&conn, "trace1", "span1", &recent1);
        insert_test_span(&conn, "trace2", "span2", &recent2);

        // Cleanup with 1 minute retention - should preserve both
        let (deleted, trace_ids) = cleanup_by_time(&conn, 1).expect("Should cleanup");
        assert_eq!(deleted, 0);
        assert!(trace_ids.is_empty());

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
            .expect("Should query");
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_cleanup_deletes_oldest_first() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Insert spans with different timestamps
        insert_test_span(&conn, "oldest", "span1", "2020-01-01 00:00:00");
        insert_test_span(&conn, "middle", "span2", "2020-06-01 00:00:00");
        insert_test_span(&conn, "newest", "span3", "2020-12-01 00:00:00");

        // Use delete_oldest_spans directly to verify ordering
        let (deleted, _trace_ids) = delete_oldest_spans(&conn, 1).expect("Should delete");
        assert_eq!(deleted, 1);

        // Verify oldest was deleted
        let oldest_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM otel_spans WHERE trace_id = 'oldest'",
                [],
                |row| row.get(0),
            )
            .expect("Should query");
        assert_eq!(oldest_exists, 0);

        // Middle and newest should still exist
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
            .expect("Should query");
        assert_eq!(remaining, 2);
    }

    #[tokio::test]
    async fn test_cleanup_mixed_old_and_new_spans() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Insert mix of old and new spans
        insert_test_span(&conn, "old1", "span1", "2020-01-01 00:00:00");
        insert_test_span(&conn, "old2", "span2", "2020-01-02 00:00:00");
        insert_test_span(&conn, "old3", "span3", "2020-01-03 00:00:00");

        let now = Utc::now();
        insert_test_span(
            &conn,
            "new1",
            "span4",
            &now.format("%Y-%m-%d %H:%M:%S%.6f").to_string(),
        );
        insert_test_span(
            &conn,
            "new2",
            "span5",
            &(now - TimeDelta::seconds(10))
                .format("%Y-%m-%d %H:%M:%S%.6f")
                .to_string(),
        );

        // Cleanup old spans
        let (deleted, _trace_ids) = cleanup_by_time(&conn, 1).expect("Should cleanup");
        assert_eq!(deleted, 3);

        // Only new spans should remain
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
            .expect("Should query");
        assert_eq!(count, 2);
    }

    // ========================================================================
    // METRICS RETENTION TESTS
    // ========================================================================

    fn insert_test_metric(conn: &Connection, name: &str, timestamp: &str) {
        conn.execute(
            "INSERT INTO otel_metrics (metric_name, metric_type, timestamp)
             VALUES (?1, 'gauge', ?2)",
            [name, timestamp],
        )
        .expect("Failed to insert test metric");
    }

    #[tokio::test]
    async fn test_cleanup_metrics_by_time_empty_table() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        let deleted = cleanup_metrics_by_time(&conn, 60).expect("Should cleanup");
        assert_eq!(deleted, 0);
    }

    #[tokio::test]
    async fn test_cleanup_metrics_by_time_removes_old_metrics() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Insert metrics: 2 old, 1 recent
        insert_test_metric(&conn, "metric1", "2020-01-01 00:00:00");
        insert_test_metric(&conn, "metric2", "2020-01-02 00:00:00");
        let recent = Utc::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string();
        insert_test_metric(&conn, "metric3", &recent);

        // Cleanup metrics older than 1 minute
        let deleted = cleanup_metrics_by_time(&conn, 1).expect("Should cleanup");
        assert_eq!(deleted, 2);

        // Verify only recent metric remains
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM otel_metrics", [], |row| row.get(0))
            .expect("Should query");
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_cleanup_metrics_preserves_recent_metrics() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Insert only recent metrics
        let now = Utc::now();
        let recent1 = now.format("%Y-%m-%d %H:%M:%S%.6f").to_string();
        let recent2 = (now - TimeDelta::seconds(30))
            .format("%Y-%m-%d %H:%M:%S%.6f")
            .to_string();

        insert_test_metric(&conn, "metric1", &recent1);
        insert_test_metric(&conn, "metric2", &recent2);

        // Cleanup with 1 minute retention - should preserve both
        let deleted = cleanup_metrics_by_time(&conn, 1).expect("Should cleanup");
        assert_eq!(deleted, 0);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM otel_metrics", [], |row| row.get(0))
            .expect("Should query");
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_run_retention_cleans_both_spans_and_metrics() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Insert old spans and metrics
        insert_test_span(&conn, "trace1", "span1", "2020-01-01 00:00:00");
        insert_test_span(&conn, "trace2", "span2", "2020-01-02 00:00:00");
        insert_test_metric(&conn, "metric1", "2020-01-01 00:00:00");
        insert_test_metric(&conn, "metric2", "2020-01-02 00:00:00");

        // Run retention with 1 minute limit
        let config = RetentionConfig {
            max_age_minutes: Some(1),
            max_spans: None,
        };
        let result = run_retention(&conn, &config).expect("Should run retention");
        assert!(result.deleted_count > 0);

        // Verify both spans and metrics are deleted
        let span_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
            .expect("Should query");
        assert_eq!(span_count, 0);

        let metric_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM otel_metrics", [], |row| row.get(0))
            .expect("Should query");
        assert_eq!(metric_count, 0);
    }
}
