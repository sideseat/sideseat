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
        insert_metrics(conn, metrics)?;
        Ok(())
    })
}

fn insert_metrics(conn: &Connection, metrics: &[NormalizedMetric]) -> Result<(), DuckdbError> {
    if metrics.is_empty() {
        return Ok(());
    }

    let mut appender = conn.appender("otel_metrics")?;

    for m in metrics {
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
