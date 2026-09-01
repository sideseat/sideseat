//! DuckDB metric repository using Appender API
//!
//! Provides high-throughput batch writes for normalized metrics.

use duckdb::Connection;
use duckdb::params;

use crate::data::duckdb::sql_types::{SqlOptTimestamp, SqlTimestamp};
use crate::data::duckdb::{DuckdbError, NormalizedMetric, in_transaction};
use crate::utils::json::json_to_opt_string;

pub fn insert_batch(conn: &Connection, metrics: &[NormalizedMetric]) -> Result<(), DuckdbError> {
    if metrics.is_empty() {
        return Ok(());
    }

    in_transaction(conn, |conn| {
        // A re-delivery *replaces* its datapoints rather than joining them.
        //
        // The table is append-only, and counting distinct ids at read time hid the duplicates from the
        // deletion check without removing them: two rows for one datapoint remained, holding two possibly
        // different measurements of the same instant with nothing to say which is current, and the write
        // amplification of a retrying exporter was unbounded. That is the same failure ClickHouse's
        // `ReplacingMergeTree` avoids by construction, so DuckDB does it explicitly - deleting the ids
        // about to be written, in the same transaction as the append, which is what makes it atomic.
        //
        // Spans already follow this rule: a re-delivered span id overwrites. `datapoint_id` is what lets
        // metrics follow it - see `domain::metrics::identity`.
        replace_existing(conn, metrics)?;
        insert_metrics(conn, metrics)?;
        Ok(())
    })
}

/// Delete any rows already stored for the datapoints about to be written.
///
/// Chunked, because a batch can carry many datapoints and a parameter list has a practical limit. Rows
/// written before the identity existed carry `''`, which never appears in this list - every datapoint that
/// reaches here has a real id - so legacy rows are untouched.
fn replace_existing(conn: &Connection, metrics: &[NormalizedMetric]) -> Result<(), DuckdbError> {
    const CHUNK: usize = 500;
    for chunk in metrics.chunks(CHUNK) {
        let mut by_project: std::collections::HashMap<&str, Vec<&str>> =
            std::collections::HashMap::new();
        for m in chunk {
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

fn insert_metrics(conn: &Connection, metrics: &[NormalizedMetric]) -> Result<(), DuckdbError> {
    if metrics.is_empty() {
        return Ok(());
    }

    let mut appender = conn.appender("otel_metrics")?;

    // Within one batch too, and last-wins.
    //
    // `replace_existing` removes what is already *stored*, which leaves a batch that carries the same
    // datapoint twice - a retrying exporter that re-sends part of a payload, or an SDK that flushes an
    // overlapping window - appending both rows. The delete cannot catch that, because neither row existed
    // when it ran. Keeping the last occurrence matches the rule everywhere else: a later delivery of a
    // datapoint replaces an earlier one.
    // An *empty* id is "no identity known", never "the same datapoint". Legacy rows carry `''` for that
    // reason, and so does anything written without going through the extractor that stamps identities.
    // Collapsing on it made every such datapoint one row - which a test writing three metrics by hand
    // caught at once, and which a caller in production would not have.
    let mut last_by_identity: std::collections::HashMap<(&str, &str), usize> =
        std::collections::HashMap::with_capacity(metrics.len());
    for (index, m) in metrics.iter().enumerate() {
        if m.datapoint_id.is_empty() {
            continue;
        }
        last_by_identity.insert(
            (
                m.project_id.as_deref().unwrap_or(""),
                m.datapoint_id.as_str(),
            ),
            index,
        );
    }

    for (index, m) in metrics.iter().enumerate() {
        if !m.datapoint_id.is_empty()
            && last_by_identity.get(&(
                m.project_id.as_deref().unwrap_or(""),
                m.datapoint_id.as_str(),
            )) != Some(&index)
        {
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
        ])?;
    }

    appender.flush()?;
    drop(appender);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::storage::AppStorage;
    use crate::data::duckdb::{DuckdbService, MetricType};
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
