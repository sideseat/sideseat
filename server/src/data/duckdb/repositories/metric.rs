//! DuckDB metric repository using Appender API
//!
//! Provides high-throughput batch writes for normalized metrics.

use duckdb::Connection;
use duckdb::params;

use crate::data::duckdb::sql_types::{SqlOptTimestamp, SqlTimestamp};
use crate::data::duckdb::{DuckdbError, NormalizedMetric, in_transaction};
use sideseat_core::utils::json::json_to_opt_string;

pub fn insert_batch(conn: &Connection, metrics: &[NormalizedMetric]) -> Result<(), DuckdbError> {
    if metrics.is_empty() {
        return Ok(());
    }

    // One `now` for the whole batch, so a row whose `ingested_at` is `None` compares against storage and
    // against its peers using the same value it will be stamped with.
    let batch_now = chrono::Utc::now();

    in_transaction(conn, |conn| {
        // A re-delivery *replaces* its datapoints rather than joining them, and **the higher
        // `ingested_at` wins** - not whichever committed last.
        //
        // The table is append-only, and counting distinct ids at read time hid the duplicates from the
        // deletion check without removing them: two rows for one datapoint remained, holding two possibly
        // different measurements of the same instant with nothing to say which is current, and the write
        // amplification of a retrying exporter was unbounded. That is the same failure ClickHouse's
        // `ReplacingMergeTree` avoids by construction, so DuckDB does it explicitly - deleting the ids
        // about to be written, in the same transaction as the append, which is what makes it atomic.
        //
        // Comparing versions rather than always overwriting is what makes the two backends agree.
        // ClickHouse keeps the row with the highest version, so an unconditional replace here meant a
        // clock-regressed correction won on DuckDB and lost on ClickHouse: one corrected datapoint, two
        // different measurements depending on which backend served the read.
        //
        // Spans already follow this rule: a re-delivered span id overwrites. `datapoint_id` is what lets
        // metrics follow it - see `domain::metrics::identity`.
        let winners = winning_indices(conn, metrics, batch_now)?;
        replace_existing(conn, metrics, &winners)?;
        insert_metrics(conn, metrics, &winners, batch_now)?;
        Ok(())
    })
}

/// The indices of the rows that should actually be written: the highest-versioned occurrence of each
/// identity in the batch, minus any that lose to what is already stored.
///
/// Ties go to the **later occurrence**, which is ClickHouse's rule for an equal version (the most
/// recently inserted row wins), so a batch carrying one datapoint twice at the same instant resolves the
/// same way on both backends.
fn winning_indices(
    conn: &Connection,
    metrics: &[NormalizedMetric],
    batch_now: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<bool>, DuckdbError> {
    let version = |m: &NormalizedMetric| m.ingested_at.unwrap_or(batch_now);
    let mut keep = vec![true; metrics.len()];

    // An *empty* id is "no identity known", never "the same datapoint" - legacy rows carry `''`, and so
    // does anything written without passing through the extractor that stamps identities. Collapsing on
    // it made every such datapoint one row.
    let mut best: std::collections::HashMap<(&str, &str), usize> =
        std::collections::HashMap::with_capacity(metrics.len());
    for (index, m) in metrics.iter().enumerate() {
        if m.datapoint_id.is_empty() {
            continue;
        }
        let key = (
            m.project_id.as_deref().unwrap_or(""),
            m.datapoint_id.as_str(),
        );
        match best.get(&key) {
            // `>=` so an equal version prefers the later occurrence.
            Some(&prev) if version(&metrics[prev]) > version(m) => keep[index] = false,
            Some(&prev) => {
                keep[prev] = false;
                best.insert(key, index);
            }
            None => {
                best.insert(key, index);
            }
        }
    }

    // Now drop the batch's winners that lose to a stored row.
    //
    // Read in **chunks**, one statement per chunk, not one per identity. A query per distinct datapoint
    // meant a batch of ten thousand datapoints issued ten thousand round trips inside the write
    // transaction - a throughput regression against the chunked delete this function sits in front of,
    // and on the hot ingestion path.
    //
    // Compared as epoch microseconds: `chrono::DateTime` is not `FromSql` here, and micros are the
    // column's own resolution, so nothing is lost by the conversion.
    let mut stored: std::collections::HashMap<(String, String), i64> =
        std::collections::HashMap::with_capacity(best.len());
    let keys: Vec<(&str, &str)> = best.keys().copied().collect();
    const PROBE_CHUNK: usize = 500;
    for chunk in keys.chunks(PROBE_CHUNK) {
        // Grouped by project so the predicate stays `project_id = ? AND datapoint_id IN (…)`, which is
        // what the primary key can serve; a flat `OR` over pairs cannot use it.
        let mut by_project: std::collections::HashMap<&str, Vec<&str>> =
            std::collections::HashMap::new();
        for (project, id) in chunk {
            by_project.entry(project).or_default().push(id);
        }
        for (project, ids) in by_project {
            let placeholders = vec!["?"; ids.len()].join(", ");
            let sql = format!(
                "SELECT datapoint_id, epoch_us(ingested_at) FROM otel_metrics \
                 WHERE project_id = ? AND datapoint_id IN ({placeholders})"
            );
            let mut params: Vec<&str> = Vec::with_capacity(ids.len() + 1);
            params.push(project);
            params.extend(ids.iter().copied());
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(duckdb::params_from_iter(params), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (id, version_us) = row?;
                // The table holds at most one row per identity, so a later duplicate would be a bug
                // elsewhere; take the greatest defensively rather than assuming.
                stored
                    .entry((project.to_string(), id))
                    .and_modify(|existing| *existing = (*existing).max(version_us))
                    .or_insert(version_us);
            }
        }
    }

    for (&(project, id), &index) in &best {
        if let Some(&stored_us) = stored.get(&(project.to_string(), id.to_string()))
            && stored_us > version(&metrics[index]).timestamp_micros()
        {
            keep[index] = false;
        }
    }

    Ok(keep)
}

/// Delete any rows already stored for the datapoints about to be written.
///
/// Chunked, because a batch can carry many datapoints and a parameter list has a practical limit. Rows
/// written before the identity existed carry `''`, which never appears in this list - every datapoint that
/// reaches here has a real id - so legacy rows are untouched.
fn replace_existing(
    conn: &Connection,
    metrics: &[NormalizedMetric],
    keep: &[bool],
) -> Result<(), DuckdbError> {
    const CHUNK: usize = 500;
    for (chunk_index, chunk) in metrics.chunks(CHUNK).enumerate() {
        let offset = chunk_index * CHUNK;
        let mut by_project: std::collections::HashMap<&str, Vec<&str>> =
            std::collections::HashMap::new();
        for (within, m) in chunk.iter().enumerate() {
            // Only for a row that is actually about to be written. Deleting for a *loser* would remove
            // the stored winner and put nothing back.
            if !keep[offset + within] {
                continue;
            }
            // Empty means "no identity known" - legacy rows carry it, and deleting `datapoint_id = ''`
            // would take every one of them on the first write after an upgrade.
            if m.datapoint_id.is_empty() {
                continue;
            }
            by_project
                .entry(m.project_id.as_deref().unwrap_or(""))
                .or_default()
                .push(m.datapoint_id.as_str());
        }
        for (project, mut ids) in by_project {
            if ids.is_empty() {
                continue;
            }
            ids.sort_unstable();
            ids.dedup();
            let placeholders = vec!["?"; ids.len()].join(", ");
            let sql = format!(
                "DELETE FROM otel_metrics WHERE project_id = ? AND datapoint_id IN ({placeholders})"
            );
            let mut params: Vec<&str> = Vec::with_capacity(ids.len() + 1);
            params.push(project);
            params.extend(ids);
            conn.execute(&sql, duckdb::params_from_iter(params))?;
        }
    }
    Ok(())
}

fn insert_metrics(
    conn: &Connection,
    metrics: &[NormalizedMetric],
    keep: &[bool],
    batch_now: chrono::DateTime<chrono::Utc>,
) -> Result<(), DuckdbError> {
    if metrics.is_empty() {
        return Ok(());
    }

    let mut appender = conn.appender("otel_metrics")?;

    // Within one batch too, and **highest version wins**.
    //
    // `replace_existing` removes what is already *stored*, which leaves a batch that carries the same
    // datapoint twice - a retrying exporter that re-sends part of a payload, or an SDK that flushes an
    // overlapping window - appending both rows. The delete cannot catch that, because neither row existed
    // when it ran. `winning_indices` resolves it before we get here, by the same rule the stored
    // comparison uses, so one datapoint appears at most once whatever the batch contained.
    for (index, m) in metrics.iter().enumerate() {
        if !keep[index] {
            continue;
        }
        // Column order must match schema.rs CREATE TABLE definition
        appender.append_row(params![
            // IDENTITY
            m.project_id.as_deref(),
            m.metric_name.as_str(),
            m.metric_description.as_deref(),
            m.metric_unit.as_deref(),
            // METRIC TYPE & AGGREGATION
            m.metric_type.as_str(),
            m.aggregation_temporality.as_str(),
            m.is_monotonic,
            // TIMING
            SqlTimestamp(m.timestamp),
            SqlOptTimestamp(m.start_timestamp),
            // VALUE (for Gauge/Sum)
            m.value_int,
            m.value_double,
            // HISTOGRAM AGGREGATES
            m.histogram_count.map(|c| c as i64),
            m.histogram_sum,
            m.histogram_min,
            m.histogram_max,
            json_to_opt_string(&m.histogram_bucket_counts).as_deref(),
            json_to_opt_string(&m.histogram_explicit_bounds).as_deref(),
            // EXPONENTIAL HISTOGRAM
            m.exp_histogram_scale,
            m.exp_histogram_zero_count.map(|c| c as i64),
            m.exp_histogram_zero_threshold,
            json_to_opt_string(&m.exp_histogram_positive).as_deref(),
            json_to_opt_string(&m.exp_histogram_negative).as_deref(),
            // SUMMARY
            m.summary_count.map(|c| c as i64),
            m.summary_sum,
            json_to_opt_string(&m.summary_quantiles).as_deref(),
            // EXEMPLAR
            m.exemplar_trace_id.as_deref(),
            m.exemplar_span_id.as_deref(),
            m.exemplar_value_int,
            m.exemplar_value_double,
            SqlOptTimestamp(m.exemplar_timestamp),
            json_to_opt_string(&m.exemplar_attributes).as_deref(),
            // CONTEXT
            m.session_id.as_deref(),
            m.user_id.as_deref(),
            m.environment.as_deref(),
            // RESOURCE
            m.service_name.as_deref(),
            m.service_version.as_deref(),
            m.service_namespace.as_deref(),
            m.service_instance_id.as_deref(),
            // INSTRUMENTATION SCOPE
            m.scope_name.as_deref(),
            m.scope_version.as_deref(),
            // ATTRIBUTES
            json_to_opt_string(&m.attributes).as_deref(),
            json_to_opt_string(&m.resource_attributes).as_deref(),
            // FLAGS & RAW
            m.flags as i32,
            json_to_opt_string(&m.raw_metric).as_deref(),
            // IDENTITY, last - see the schema comment. The appender is positional and `ALTER TABLE ADD
            // COLUMN` appends, so this is the only position a fresh and an upgraded database share.
            m.datapoint_id.as_str(),
            json_to_opt_string(&m.scope_attributes).as_deref(),
            m.scope_schema_url.as_deref(),
            m.resource_schema_url.as_deref(),
            json_to_opt_string(&m.exemplars).as_deref(),
            // The version, appended last for the same positional reason as everything above it.
            SqlTimestamp(m.ingested_at.unwrap_or(batch_now)),
        ])?;
    }

    appender.flush()?;
    drop(appender);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::duckdb::{DuckdbService, MetricType};
    use chrono::Utc;
    use sideseat_core::core::storage::AppStorage;
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

    /// A datapoint at a given version, for the version-resolution tests below.
    fn datapoint(
        id: &str,
        value: f64,
        ingested_at: Option<chrono::DateTime<Utc>>,
    ) -> NormalizedMetric {
        NormalizedMetric {
            project_id: Some("default".to_string()),
            datapoint_id: id.to_string(),
            metric_name: "cpu".to_string(),
            metric_type: MetricType::Gauge,
            timestamp: Utc::now(),
            value_double: Some(value),
            ingested_at,
            ..Default::default()
        }
    }

    fn stored_value(conn: &Connection, id: &str) -> Option<f64> {
        conn.query_row(
            "SELECT value_double FROM otel_metrics WHERE datapoint_id = ?1",
            [id],
            |row| row.get(0),
        )
        .ok()
    }

    fn row_count(conn: &Connection, id: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM otel_metrics WHERE datapoint_id = ?1",
            [id],
            |row| row.get(0),
        )
        .expect("count")
    }

    /// A **clock-regressed** re-delivery must not overwrite a newer stored version.
    ///
    /// This is the case the whole `winning_indices` comparison exists for. DuckDB used to delete and
    /// re-insert unconditionally, so whichever delivery committed last won - while ClickHouse keeps the
    /// highest `ingested_at`. One corrected datapoint could therefore read differently per backend.
    #[tokio::test]
    async fn a_clock_regressed_redelivery_does_not_overwrite_a_newer_stored_version() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        let newer = Utc::now();
        let older = newer - chrono::TimeDelta::hours(1);

        insert_batch(&conn, &[datapoint("dp1", 2.0, Some(newer))]).expect("first write");
        // Arrives later in wall-clock order but carries an earlier version: it loses.
        insert_batch(&conn, &[datapoint("dp1", 1.0, Some(older))]).expect("second write");

        assert_eq!(row_count(&conn, "dp1"), 1, "still one row per identity");
        assert_eq!(
            stored_value(&conn, "dp1"),
            Some(2.0),
            "the higher version must survive; an unconditional replace kept the regressed one"
        );
    }

    /// A genuine correction - a higher version - does replace.
    #[tokio::test]
    async fn a_higher_versioned_correction_replaces_the_stored_row() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        let first = Utc::now() - chrono::TimeDelta::hours(1);
        let corrected = Utc::now();

        insert_batch(&conn, &[datapoint("dp1", 1.0, Some(first))]).expect("first write");
        insert_batch(&conn, &[datapoint("dp1", 9.0, Some(corrected))]).expect("correction");

        assert_eq!(row_count(&conn, "dp1"), 1);
        assert_eq!(stored_value(&conn, "dp1"), Some(9.0), "the correction wins");
    }

    /// Within one batch the highest version wins, whatever order the rows arrive in.
    #[tokio::test]
    async fn within_one_batch_the_highest_version_wins_regardless_of_order() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        let high = Utc::now();
        let low = high - chrono::TimeDelta::hours(1);

        // The winner is listed *first*, so "last occurrence wins" would pick the wrong one.
        insert_batch(
            &conn,
            &[
                datapoint("dp1", 7.0, Some(high)),
                datapoint("dp1", 3.0, Some(low)),
            ],
        )
        .expect("write");

        assert_eq!(row_count(&conn, "dp1"), 1);
        assert_eq!(stored_value(&conn, "dp1"), Some(7.0));
    }

    /// Datapoints with no identity are never collapsed onto each other.
    #[tokio::test]
    async fn identity_less_datapoints_are_not_collapsed() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        let now = Utc::now();
        insert_batch(
            &conn,
            &[
                datapoint("", 1.0, Some(now)),
                datapoint("", 2.0, Some(now)),
                datapoint("", 3.0, Some(now)),
            ],
        )
        .expect("write");

        assert_eq!(
            row_count(&conn, ""),
            3,
            "an empty id is 'no identity known', not 'the same datapoint'"
        );
    }

    #[tokio::test]
    async fn test_insert_empty_batch() {
        let (_temp_dir, analytics) = create_test_service().await;

        let conn = analytics.conn();
        let result = insert_batch(&conn, &[]);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_insert_gauge_metric() {
        let (_temp_dir, analytics) = create_test_service().await;

        let metric = NormalizedMetric {
            metric_name: "test.gauge".to_string(),
            metric_type: MetricType::Gauge,
            timestamp: Utc::now(),
            value_double: Some(42.0),
            ..Default::default()
        };

        {
            let conn = analytics.conn();
            let result = insert_batch(&conn, &[metric]);
            assert!(result.is_ok());
        }

        let conn = analytics.conn();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM otel_metrics WHERE metric_name = 'test.gauge'",
                [],
                |row| row.get(0),
            )
            .expect("Should query");
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_insert_sum_metric() {
        let (_temp_dir, analytics) = create_test_service().await;

        let metric = NormalizedMetric {
            project_id: Some("test-project".to_string()),
            metric_name: "test.counter".to_string(),
            metric_type: MetricType::Sum,
            timestamp: Utc::now(),
            value_int: Some(100),
            is_monotonic: Some(true),
            ..Default::default()
        };

        {
            let conn = analytics.conn();
            let result = insert_batch(&conn, &[metric]);
            assert!(result.is_ok());
        }

        let conn = analytics.conn();
        let (name, is_monotonic): (String, Option<bool>) = conn
            .query_row(
                "SELECT metric_name, is_monotonic FROM otel_metrics WHERE project_id = 'test-project'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("Should query");
        assert_eq!(name, "test.counter");
        assert_eq!(is_monotonic, Some(true));
    }

    #[tokio::test]
    async fn test_insert_histogram_metric() {
        let (_temp_dir, analytics) = create_test_service().await;

        let metric = NormalizedMetric {
            metric_name: "test.histogram".to_string(),
            metric_type: MetricType::Histogram,
            timestamp: Utc::now(),
            histogram_count: Some(100),
            histogram_sum: Some(500.0),
            histogram_min: Some(1.0),
            histogram_max: Some(10.0),
            histogram_bucket_counts: serde_json::json!([10, 20, 30, 40]),
            histogram_explicit_bounds: serde_json::json!([1.0, 5.0, 10.0]),
            ..Default::default()
        };

        {
            let conn = analytics.conn();
            let result = insert_batch(&conn, &[metric]);
            assert!(result.is_ok());
        }

        let conn = analytics.conn();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM otel_metrics WHERE metric_name = 'test.histogram' AND metric_type = 'histogram'",
                [],
                |row| row.get(0),
            )
            .expect("Should query");
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_insert_batch_multiple_metrics() {
        let (_temp_dir, analytics) = create_test_service().await;

        let metrics = vec![
            NormalizedMetric {
                metric_name: "batch.metric1".to_string(),
                metric_type: MetricType::Gauge,
                timestamp: Utc::now(),
                value_double: Some(1.0),
                ..Default::default()
            },
            NormalizedMetric {
                metric_name: "batch.metric2".to_string(),
                metric_type: MetricType::Gauge,
                timestamp: Utc::now(),
                value_double: Some(2.0),
                ..Default::default()
            },
            NormalizedMetric {
                metric_name: "batch.metric3".to_string(),
                metric_type: MetricType::Sum,
                timestamp: Utc::now(),
                value_int: Some(3),
                ..Default::default()
            },
        ];

        {
            let conn = analytics.conn();
            let result = insert_batch(&conn, &metrics);
            assert!(result.is_ok());
        }

        let conn = analytics.conn();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM otel_metrics WHERE metric_name LIKE 'batch.%'",
                [],
                |row| row.get(0),
            )
            .expect("Should query");
        assert_eq!(count, 3);
    }
}
