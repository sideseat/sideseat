//! DuckDB retention management
//!
//! Efficient batch deletion with transaction-safe cascading deletes.
//! Note: DuckDB doesn't support data-modifying CTEs, so we use explicit transactions.

use std::collections::HashMap;

use chrono::{TimeDelta, Utc};
use duckdb::Connection;

use super::{DuckdbError, in_transaction};
use crate::core::config::RetentionConfig;
use crate::data::duckdb::repositories::query::DEDUP_SPANS;

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

/// Cap on trace IDs collected from one batch, as a guard against a pathological batch rather than a
/// working limit.
///
/// It must stay **at or above [`RETENTION_BATCH_SIZE`]**, because in the worst case every span in a
/// batch belongs to its own trace. A lower value silently truncated the cleanup list: a batch could
/// delete 100 000 spans while only 10 000 traces were handed to file and favourite cleanup, and the
/// omitted traces never reappeared in a later pass - their spans were already gone - so their file
/// associations, ref-counted bytes and favourites were orphaned permanently. The memory this bounds
/// is a few megabytes at the full batch size, which is not the explosion the old value implied.
const MAX_TRACE_IDS_PER_CYCLE: usize = RETENTION_BATCH_SIZE as usize;

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
        let batch = delete_spans_before(conn, &cutoff_str, RETENTION_BATCH_SIZE)?;
        if batch.identities == 0 {
            break;
        }
        tracing::debug!(
            identities = batch.identities,
            rows = batch.rows,
            "Deleted batch of expired spans"
        );
        total_deleted += batch.rows;
        merge_trace_ids(&mut all_trace_ids, batch.trace_ids_by_project);
    }
    Ok((total_deleted, all_trace_ids))
}

/// Max batches per count-based cleanup cycle (prevents unbounded blocking)
const MAX_COUNT_CLEANUP_BATCHES: usize = 10;

/// Execute retention based on max span count, **per project**.
///
/// `max_spans` is a limit on each project, not on the deployment. Counting the whole table made it a
/// shared budget that one noisy tenant exhausted on everyone's behalf, and the spans deleted to make
/// room belonged to whichever project happened to hold the globally oldest rows - so a quiet tenant
/// lost data because a busy one was over.
///
/// The count is over the **winning** relation. Counting raw rows double-counts an identity that has
/// been corrected, so a project one span over the limit appeared two over, the sweep deleted both
/// identities, and it finished *below* the limit.
///
/// Returns (spans_deleted, trace_ids_by_project). Iterates in batches with a limit to prevent
/// unbounded blocking.
pub fn cleanup_by_count(
    conn: &Connection,
    max_spans: u64,
) -> Result<(u64, HashMap<String, Vec<String>>), DuckdbError> {
    let max_spans_i64 = i64::try_from(max_spans).unwrap_or(i64::MAX);

    let over_limit = match projects_over_limit(conn, max_spans_i64) {
        Ok(projects) => projects,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to query per-project span counts");
            return Ok((0, HashMap::new()));
        }
    };

    if over_limit.is_empty() {
        tracing::debug!(max_spans, "Every project within its span limit");
        return Ok((0, HashMap::new()));
    }

    let mut total_deleted = 0u64;
    let mut all_trace_ids: HashMap<String, Vec<String>> = HashMap::new();

    for (project_id, span_count) in over_limit {
        let overage = span_count - max_spans_i64;
        tracing::debug!(
            %project_id,
            span_count,
            max_spans,
            to_delete = overage,
            "Project exceeds its span limit, cleaning up"
        );
        let (deleted, trace_ids) =
            trim_project_to_limit(conn, &project_id, overage, RETENTION_BATCH_SIZE)?;
        total_deleted += deleted;
        merge_trace_ids(&mut all_trace_ids, trace_ids);
    }

    Ok((total_deleted, all_trace_ids))
}

/// Delete `overage` of one project's oldest span identities, in batches of `batch_size`.
///
/// Separate from [`cleanup_by_count`] so the loop's progress arithmetic is reachable in a test with a
/// small `batch_size`; at the production value an overage large enough to need two batches is 100 000
/// identities, which no unit test is going to build.
fn trim_project_to_limit(
    conn: &Connection,
    project_id: &str,
    overage: i64,
    batch_size: i64,
) -> Result<(u64, HashMap<String, Vec<String>>), DuckdbError> {
    let mut total_deleted = 0u64;
    let mut all_trace_ids: HashMap<String, Vec<String>> = HashMap::new();
    let mut remaining = overage;

    for _ in 0..MAX_COUNT_CLEANUP_BATCHES {
        if remaining <= 0 {
            break;
        }
        let limit = remaining.min(batch_size);
        let batch = delete_oldest_spans_for_project(conn, project_id, limit)?;
        if batch.identities == 0 {
            break;
        }
        tracing::debug!(
            %project_id,
            identities = batch.identities,
            rows = batch.rows,
            "Deleted batch of excess spans"
        );
        total_deleted += batch.rows;
        merge_trace_ids(&mut all_trace_ids, batch.trace_ids_by_project);
        // Identities, not rows. One identity can take several revisions with it, so subtracting the
        // row count overshoots the remainder and stops the loop while the project is still over its
        // limit.
        remaining = remaining.saturating_sub(batch.identities as i64);
    }

    Ok((total_deleted, all_trace_ids))
}

/// Projects whose winning span count exceeds `max_spans`, with that count.
fn projects_over_limit(
    conn: &Connection,
    max_spans: i64,
) -> Result<Vec<(String, i64)>, DuckdbError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT project_id, COUNT(*) AS span_count FROM {DEDUP_SPANS}
         GROUP BY project_id
         HAVING COUNT(*) > ?1
         ORDER BY project_id"
    ))?;
    let rows = stmt.query_map([max_spans], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Outcome of one retention batch.
struct BatchOutcome {
    /// Span *identities* selected for deletion. Progress is counted in these, never in deleted
    /// rows: deleting one identity removes every revision of it, so a row count overshoots the
    /// caller's remaining budget and stops a count-based sweep while it is still over the limit.
    identities: u64,
    /// Physical rows removed, which is what the caller reports.
    rows: u64,
    trace_ids_by_project: HashMap<String, Vec<String>>,
}

/// Delete spans before cutoff timestamp (for time-based retention)
fn delete_spans_before(
    conn: &Connection,
    cutoff: &str,
    limit: i64,
) -> Result<BatchOutcome, DuckdbError> {
    delete_spans_with_query(
        conn,
        &format!(
            "INSERT INTO _retention_batch
             SELECT project_id, trace_id, span_id FROM {DEDUP_SPANS}
             WHERE timestamp_start < ?1
             ORDER BY timestamp_start ASC
             LIMIT ?2"
        ),
        &[&cutoff as &dyn duckdb::ToSql, &limit],
    )
}

/// Delete the oldest N spans **of one project** (for count-based retention).
fn delete_oldest_spans_for_project(
    conn: &Connection,
    project_id: &str,
    limit: i64,
) -> Result<BatchOutcome, DuckdbError> {
    delete_spans_with_query(
        conn,
        &format!(
            "INSERT INTO _retention_batch
             SELECT project_id, trace_id, span_id FROM {DEDUP_SPANS}
             WHERE project_id = ?1
             ORDER BY timestamp_start ASC
             LIMIT ?2"
        ),
        &[&project_id as &dyn duckdb::ToSql, &limit],
    )
}

/// Common delete logic using a temp table for efficiency.
///
/// Two properties this must not lose:
///
/// **The batch is keyed by `(project_id, trace_id, span_id)`.** A span id is unique only within a
/// trace and a trace id only within a project, and both come from the client - so a batch keyed by
/// `(trace_id, span_id)` alone let expiring one tenant's span delete another tenant's span, or a
/// held one, whenever two projects presented the same trace id.
///
/// **Candidates come from the deduplicated relation, not the raw table.** `otel_spans` is
/// append-only, so an expired *old* revision would otherwise select an identity whose winning
/// correction is recent, and the delete - which removes every revision of the identity - would take
/// the correction with it. Reads already go through [`DEDUP_SPANS`]; retention has to agree with them.
fn delete_spans_with_query(
    conn: &Connection,
    insert_sql: &str,
    params: &[&dyn duckdb::ToSql],
) -> Result<BatchOutcome, DuckdbError> {
    in_transaction(conn, |conn| {
        conn.execute(
            "CREATE TEMP TABLE IF NOT EXISTS _retention_batch (
                project_id VARCHAR NOT NULL,
                trace_id VARCHAR NOT NULL,
                span_id VARCHAR NOT NULL,
                PRIMARY KEY (project_id, trace_id, span_id)
            )",
            [],
        )?;
        conn.execute("DELETE FROM _retention_batch", [])?;

        let identities = conn.execute(insert_sql, params)? as u64;

        // Collected BEFORE the deletion: afterwards the rows naming these traces are gone.
        let trace_ids_by_project = collect_trace_ids_for_cleanup(conn)?;

        // Removes every revision of each selected identity (events, links and messages are
        // embedded in the span row).
        let rows = conn.execute(
            "DELETE FROM otel_spans
             WHERE (project_id, trace_id, span_id)
                   IN (SELECT project_id, trace_id, span_id FROM _retention_batch)",
            [],
        )? as u64;

        Ok(BatchOutcome {
            identities,
            rows,
            trace_ids_by_project,
        })
    })
}

/// Collect distinct `(project_id, trace_id)` pairs from the retention batch for file cleanup.
///
/// Reads the batch directly - the project is a column there now, so the join back to `otel_spans`
/// that previously recovered it is gone.
fn collect_trace_ids_for_cleanup(
    conn: &Connection,
) -> Result<HashMap<String, Vec<String>>, DuckdbError> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT project_id, trace_id FROM _retention_batch
         ORDER BY project_id, trace_id
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
        insert_span_for_project(conn, "default", trace_id, span_id, timestamp);
    }

    fn insert_span_for_project(
        conn: &Connection,
        project_id: &str,
        trace_id: &str,
        span_id: &str,
        timestamp: &str,
    ) {
        conn.execute(
            "INSERT INTO otel_spans (trace_id, span_id, span_name, timestamp_start, project_id)
             VALUES (?1, ?2, 'test', ?3, ?4)",
            [trace_id, span_id, timestamp, project_id],
        )
        .expect("Failed to insert test span");
    }

    /// A second delivery of an existing span. `otel_spans` is append-only, so this leaves both rows
    /// and `ingested_at` decides which one reads see.
    fn redeliver_span(
        conn: &Connection,
        project_id: &str,
        trace_id: &str,
        span_id: &str,
        timestamp: &str,
        ingested_at: &str,
    ) {
        conn.execute(
            "INSERT INTO otel_spans
                 (trace_id, span_id, span_name, timestamp_start, project_id, ingested_at)
             VALUES (?1, ?2, 'test', ?3, ?4, ?5)",
            [trace_id, span_id, timestamp, project_id, ingested_at],
        )
        .expect("Failed to re-deliver test span");
    }

    fn span_count(conn: &Connection, project_id: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM otel_spans WHERE project_id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .expect("Should query")
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

        // Use the batch primitive directly to verify ordering
        let batch = delete_oldest_spans_for_project(&conn, "default", 1).expect("Should delete");
        assert_eq!(batch.rows, 1);

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
    // TENANT ISOLATION AND REVISION AWARENESS
    //
    // Every test below fails against the pre-fix implementation. The existing cases above cannot:
    // they use one project and one revision per span, which is exactly the shape in which both
    // defects are invisible.
    // ========================================================================

    /// A trace id comes from the client, so two projects can present the same one - and a span id is
    /// unique only within a trace. Expiring one tenant's span must not touch the other's.
    #[tokio::test]
    async fn expiring_one_project_leaves_a_colliding_span_of_another_project() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Same trace id AND same span id in two projects: the realistic collision, since both
        // values are client-supplied.
        insert_span_for_project(
            &conn,
            "alice",
            "shared-trace",
            "shared-span",
            "2020-01-01 00:00:00",
        );
        let recent = Utc::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string();
        insert_span_for_project(&conn, "bob", "shared-trace", "shared-span", &recent);

        let (deleted, trace_ids) = cleanup_by_time(&conn, 1).expect("Should cleanup");
        assert_eq!(deleted, 1, "only alice's expired span should go");

        assert_eq!(
            span_count(&conn, "alice"),
            0,
            "alice's expired span is deleted"
        );
        assert_eq!(
            span_count(&conn, "bob"),
            1,
            "bob's recent span must survive: a batch keyed only by (trace_id, span_id) deleted it"
        );

        // And the cleanup list must attribute the trace to alice alone, or bob's files are reclaimed.
        assert_eq!(trace_ids.get("alice").map(Vec::len), Some(1));
        assert!(
            !trace_ids.contains_key("bob"),
            "bob's trace must not be handed to file cleanup"
        );
    }

    /// `max_spans` is a per-project limit. A shared budget let one busy tenant's overage delete a
    /// quiet tenant's data, because the globally oldest rows are not necessarily the offender's.
    #[tokio::test]
    async fn max_spans_is_per_project_and_spends_only_the_offender() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // The *innocent* project owns the globally oldest spans. Without that arrangement a global
        // algorithm passes by luck, deleting the offender's rows because they happen to be oldest.
        // At the limit, not over it, so the only reason to touch these is a shared budget.
        for i in 0..2 {
            insert_span_for_project(
                &conn,
                "quiet",
                &format!("q{i}"),
                &format!("qs{i}"),
                &format!("2019-01-0{} 00:00:00", i + 1),
            );
        }
        for i in 0..5 {
            insert_span_for_project(
                &conn,
                "noisy",
                &format!("n{i}"),
                &format!("ns{i}"),
                &format!("2021-01-0{} 00:00:00", i + 1),
            );
        }

        let (deleted, _) = cleanup_by_count(&conn, 2).expect("Should cleanup");

        assert_eq!(
            span_count(&conn, "noisy"),
            2,
            "the over-limit project must end at exactly max_spans"
        );
        assert_eq!(
            span_count(&conn, "quiet"),
            2,
            "the at-limit project is untouched even though it owns the globally oldest spans"
        );
        assert_eq!(deleted, 3, "only noisy's three excess spans");
    }

    /// Retention selects from the deduplicated relation. Selecting raw rows let an expired *old*
    /// revision nominate an identity whose winning correction is recent - and the delete, which
    /// removes every revision of an identity, took the correction with it.
    #[tokio::test]
    async fn an_expired_revision_does_not_delete_its_recent_correction() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        let now = Utc::now();
        let recent = now.format("%Y-%m-%d %H:%M:%S%.6f").to_string();

        // First delivery is expired; the correction moves the span into the retention window.
        redeliver_span(
            &conn,
            "default",
            "trace1",
            "span1",
            "2020-01-01 00:00:00",
            "2020-01-01 00:00:00",
        );
        redeliver_span(&conn, "default", "trace1", "span1", &recent, &recent);

        let (deleted, trace_ids) = cleanup_by_time(&conn, 1).expect("Should cleanup");

        assert_eq!(
            deleted, 0,
            "the winning revision is recent, so nothing is expired"
        );
        assert_eq!(
            span_count(&conn, "default"),
            2,
            "both revisions survive: deleting the identity would have destroyed the correction"
        );
        assert!(trace_ids.is_empty());
    }

    /// Counting raw rows double-counts a corrected span, so a project one identity over the limit
    /// looked two over, both were deleted, and the sweep finished below the limit.
    #[tokio::test]
    async fn count_retention_counts_identities_not_revisions() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Two identities, one of which has been re-delivered: three physical rows, two winners.
        insert_span_for_project(&conn, "default", "t1", "s1", "2020-01-01 00:00:00");
        redeliver_span(
            &conn,
            "default",
            "t1",
            "s1",
            "2020-01-01 00:00:00",
            "2020-06-01 00:00:00",
        );
        insert_span_for_project(&conn, "default", "t2", "s2", "2020-01-02 00:00:00");

        // A limit of 2 is satisfied by two winners: nothing should be deleted.
        let (deleted, trace_ids) = cleanup_by_count(&conn, 2).expect("Should cleanup");
        assert_eq!(
            deleted, 0,
            "two winning identities are within a limit of two, whatever the revision count"
        );
        assert_eq!(span_count(&conn, "default"), 3);
        assert!(trace_ids.is_empty());
    }

    /// Progress is counted in identities. Counting deleted *rows* let one batch report more progress
    /// than it made, zeroing the remainder while the project was still over its limit.
    ///
    /// Driven through [`trim_project_to_limit`] with `batch_size = 1`, because at the production
    /// batch size an overage needing two batches is 100 000 identities. With a batch per identity,
    /// each batch deletes three rows for one identity - so row-counted progress finishes after the
    /// first batch and leaves the project two identities over.
    #[tokio::test]
    async fn count_retention_progress_is_identities_not_deleted_rows() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Five identities, each carrying three revisions.
        for i in 0..5 {
            let ts = format!("2020-01-0{} 00:00:00", i + 1);
            for rev in 0..3 {
                redeliver_span(
                    &conn,
                    "default",
                    &format!("t{i}"),
                    &format!("s{i}"),
                    &ts,
                    &format!("2020-0{}-01 00:00:00", rev + 1),
                );
            }
        }

        // Three identities over a limit of two, one identity per batch.
        let (_deleted, _) = trim_project_to_limit(&conn, "default", 3, 1).expect("Should trim");

        let winners: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {DEDUP_SPANS} WHERE project_id = 'default'"),
                [],
                |row| row.get(0),
            )
            .expect("Should query");
        assert_eq!(
            winners, 2,
            "the loop must land exactly on max_spans; row-counted progress stops early"
        );
    }

    /// Counting raw rows also makes a corrected span look like two, so the sweep overshoots.
    #[tokio::test]
    async fn count_retention_reaches_the_limit_when_identities_carry_many_revisions() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        for i in 0..2 {
            let ts = format!("2020-01-0{} 00:00:00", i + 1);
            for rev in 0..3 {
                redeliver_span(
                    &conn,
                    "default",
                    &format!("t{i}"),
                    &format!("s{i}"),
                    &ts,
                    &format!("2020-0{}-01 00:00:00", rev + 1),
                );
            }
        }
        insert_span_for_project(&conn, "default", "t2", "s2", "2020-02-01 00:00:00");
        insert_span_for_project(&conn, "default", "t3", "s3", "2020-02-02 00:00:00");

        let (_deleted, _) = cleanup_by_count(&conn, 2).expect("Should cleanup");

        let winners: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {DEDUP_SPANS} WHERE project_id = 'default'"),
                [],
                |row| row.get(0),
            )
            .expect("Should query");
        assert_eq!(winners, 2, "the sweep must land exactly on max_spans");
    }

    // ========================================================================
    // METRICS RETENTION TESTS
    // ========================================================================

    fn insert_test_metric(conn: &Connection, name: &str, timestamp: &str) {
        conn.execute(
            "INSERT INTO otel_metrics (datapoint_id, metric_name, metric_type, timestamp)
             VALUES (?1, ?1, 'gauge', ?2)",
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
