use super::raw::{enqueue_raw_for_spans, enqueue_raw_for_traces, enqueue_raw_records};
use super::retention::{
    retention_delete_expired_clickhouse, retention_select_expired, retention_select_oldest,
};
use super::*;

#[test]
fn trace_delete_is_tenant_scoped_and_parameterized() {
    let ids = vec!["trace-'one".to_string(), "trace-two".to_string()];
    for target in [
        MutationTarget::duckdb("otel_spans"),
        MutationTarget::clickhouse("otel_spans_local", " ON CLUSTER telemetry"),
    ] {
        let query = delete_traces(target, "tenant-'quoted", &ids).expect("non-empty delete");
        assert_eq!(query.operation(), QueryOperation::DeleteTraces);
        assert_eq!(query.sql().matches('?').count(), query.params().len());
        assert!(!query.sql().contains("tenant-'quoted"));
        assert!(!query.sql().contains("trace-'one"));
        assert!(query.sql().contains("project_id = ?"));
    }
}

#[test]
fn span_delete_preserves_tuple_bind_order() {
    let spans = vec![
        ("trace-a".to_string(), "span-a".to_string()),
        ("trace-b".to_string(), "span-b".to_string()),
    ];
    let query = delete_spans(MutationTarget::duckdb("otel_spans"), "p", &spans).expect("delete");
    assert_eq!(
        query.params(),
        &[
            QueryValue::String("p".to_string()),
            QueryValue::String("trace-a".to_string()),
            QueryValue::String("span-a".to_string()),
            QueryValue::String("trace-b".to_string()),
            QueryValue::String("span-b".to_string()),
        ]
    );
}

#[test]
fn clickhouse_delete_waits_for_every_replica() {
    let ids = vec!["t".to_string()];
    let query = delete_traces(
        MutationTarget::clickhouse("otel_spans_local", " ON CLUSTER prod-1"),
        "p",
        &ids,
    )
    .expect("delete");
    assert_eq!(
        query.sql(),
        "ALTER TABLE otel_spans_local ON CLUSTER prod-1 DELETE WHERE project_id = ? AND \
             trace_id IN (?) SETTINGS mutations_sync = 2"
    );
}

#[test]
fn empty_identity_sets_do_not_issue_broad_mutations() {
    assert!(delete_traces(MutationTarget::duckdb("otel_spans"), "p", &[]).is_none());
    assert!(delete_spans(MutationTarget::duckdb("otel_spans"), "p", &[]).is_none());
}

#[test]
fn project_delete_plan_covers_every_backend_table() {
    let duckdb = delete_project_data(
        MutationTarget::duckdb("otel_spans"),
        MutationTarget::duckdb("otel_metrics"),
        MutationTarget::duckdb("otel_logs"),
        MutationTarget::duckdb("otel_raw"),
        MutationTarget::duckdb("otel_raw_pending"),
        MutationTarget::duckdb("otel_raw_traces"),
        "tenant-'quoted",
    );
    assert!(duckdb.count_spans.is_none());
    assert_eq!(
        duckdb.delete_raw.sql(),
        "DELETE FROM otel_raw WHERE project_id = ?"
    );
    assert_eq!(
        duckdb.delete_raw_pending.sql(),
        "DELETE FROM otel_raw_pending WHERE project_id = ?"
    );
    assert!(duckdb.metrics_table_exists.is_none());
    assert_eq!(
        duckdb.delete_spans.sql(),
        "DELETE FROM otel_spans WHERE project_id = ?"
    );
    assert_eq!(
        duckdb.delete_metrics.sql(),
        "DELETE FROM otel_metrics WHERE project_id = ?"
    );
    assert_eq!(
        duckdb.delete_logs.sql(),
        "DELETE FROM otel_logs WHERE project_id = ?"
    );

    let clickhouse = delete_project_data(
        MutationTarget::clickhouse("otel_spans_local", " ON CLUSTER prod"),
        MutationTarget::clickhouse("otel_metrics_local", " ON CLUSTER prod"),
        MutationTarget::clickhouse("otel_logs_local", " ON CLUSTER prod"),
        MutationTarget::clickhouse("otel_raw_local", " ON CLUSTER prod"),
        MutationTarget::clickhouse("otel_raw_pending_local", " ON CLUSTER prod"),
        MutationTarget::clickhouse("otel_raw_traces_local", " ON CLUSTER prod"),
        "tenant-'quoted",
    );
    assert!(clickhouse.count_spans.is_some());
    assert!(clickhouse.metrics_table_exists.is_some());
    for statement in [
        clickhouse.count_spans.as_ref().expect("count"),
        &clickhouse.delete_spans,
        clickhouse
            .metrics_table_exists
            .as_ref()
            .expect("table check"),
        &clickhouse.delete_metrics,
        &clickhouse.delete_logs,
        &clickhouse.delete_raw,
        &clickhouse.delete_raw_pending,
        &clickhouse.delete_raw_traces,
    ] {
        assert_eq!(
            statement.sql().matches('?').count(),
            statement.params().len()
        );
        assert!(!statement.sql().contains("tenant-'quoted"));
    }
}

#[test]
fn analytical_write_targets_are_validated_capabilities() {
    let duck_span = span_write_target(Backend::Duckdb, None);
    assert_eq!(duck_span.operation(), QueryOperation::UpsertSpans);
    assert_eq!(duck_span.table(), "otel_spans");

    let click_metric = metric_write_target(Backend::Clickhouse, Some("otel_metrics_distributed"));
    assert_eq!(click_metric.operation(), QueryOperation::UpsertMetrics);
    assert_eq!(click_metric.table(), "otel_metrics_distributed");
}

#[test]
fn retention_plan_is_parameterized_and_revision_aware() {
    let cutoff = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let now = DateTime::from_timestamp(1_700_000_100, 0).unwrap();
    let expired = retention_select_expired(cutoff, now, 100);
    assert_eq!(expired.operation(), QueryOperation::EnforceRetention);
    assert_eq!(expired.sql().matches('?').count(), expired.params().len());
    assert!(expired.sql().contains("ROW_NUMBER()"));
    assert!(expired.sql().contains("hold_until"));

    let oldest = retention_select_oldest("tenant-'quoted", now, 10, 50, true);
    assert_eq!(oldest.sql().matches('?').count(), oldest.params().len());
    assert!(!oldest.sql().contains("tenant-'quoted"));
    assert!(oldest.sql().contains("hold_until"));
    assert!(
        oldest
            .sql()
            .contains("cumulative_rows <= ? OR row_rank = 1")
    );

    let clickhouse = retention_delete_expired_clickhouse(
        MutationTarget::clickhouse("otel_spans_local", " ON CLUSTER prod"),
        "tenant-'quoted",
        cutoff,
        now,
    );
    assert_eq!(
        clickhouse.sql(),
        "ALTER TABLE otel_spans_local ON CLUSTER prod DELETE WHERE timestamp_start < \
             fromUnixTimestamp64Micro(?) AND project_id = ? AND (isNull(hold_until) OR hold_until < \
             fromUnixTimestamp64Micro(?)) SETTINGS mutations_sync = 2"
    );
    assert_eq!(
        clickhouse.params(),
        &[
            QueryValue::Int64(1_700_000_000_000_000),
            QueryValue::String("tenant-'quoted".to_string()),
            QueryValue::Int64(1_700_000_100_000_000),
        ]
    );
    assert!(!clickhouse.sql().contains("tenant-'quoted"));
}

#[test]
fn metric_upsert_statements_share_identity_bind_order() {
    let ids = ["dp-'one", "dp-two"];
    let probe = metric_winner_probe("tenant-'quoted", &ids).expect("probe");
    let delete = delete_metric_winners("tenant-'quoted", &ids).expect("delete");
    assert_eq!(probe.params(), delete.params());
    assert_eq!(probe.sql().matches('?').count(), probe.params().len());
    assert_eq!(delete.sql().matches('?').count(), delete.params().len());
    for value in ["tenant-'quoted", "dp-'one", "dp-two"] {
        assert!(!probe.sql().contains(value));
        assert!(!delete.sql().contains(value));
    }
}

#[test]
fn search_backfill_is_parameterized_and_revision_guarded() {
    let document = SearchBackfillDocument {
        id: SearchRecordId::Span {
            trace_id: "trace-'quoted".to_string(),
            span_id: "span".to_string(),
        },
        expected_content_digest: Some("digest-'quoted".to_string()),
        document: SearchDocument {
            indexed: true,
            fields: [
                SearchField::Prompt,
                SearchField::Completion,
                SearchField::ToolName,
                SearchField::ToolArgs,
                SearchField::Error,
                SearchField::SpanName,
            ]
            .into_iter()
            .map(|field| sideseat_ports::types::SearchFieldTerms {
                field,
                terms: vec!["alpha".to_string()],
                truncated: false,
                text: String::new(),
            })
            .collect(),
        },
    };
    let statement = search_backfill_update(
        MutationTarget::clickhouse("otel_spans_local", " ON CLUSTER prod"),
        "tenant-'quoted",
        SearchSignal::Spans,
        &document,
    );
    assert_eq!(
        statement.sql().matches('?').count(),
        statement.params().len()
    );
    assert!(statement.sql().contains("content_digest = ?"));
    assert!(statement.sql().contains("search_indexed = 0"));
    assert!(statement.sql().contains("mutations_sync = 2"));
    for value in ["tenant-'quoted", "trace-'quoted", "digest-'quoted"] {
        assert!(!statement.sql().contains(value));
    }
}

#[test]
#[should_panic(expected = "invalid SQL mutation table")]
fn mutation_target_rejects_sql_in_table_name() {
    MutationTarget::clickhouse("otel_spans; DROP TABLE users", "");
}

#[test]
#[should_panic(expected = "invalid ClickHouse mutation cluster")]
fn mutation_target_rejects_sql_in_cluster_name() {
    MutationTarget::clickhouse("otel_spans", " ON CLUSTER prod; DROP TABLE users");
}

/// The enqueue before a trace delete reaches the records holding exactly the traces the delete removes: the
/// same predicate, bound to the same values, read from the trace index - so no deleted span's record can miss
/// reconciliation, whether or not a row still names it. A span delete reaches its spans' traces.
#[test]
fn raw_enqueue_uses_the_predicate_of_the_delete_it_precedes() {
    let traces = vec!["t1".to_string(), "t'2".to_string()];
    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let target = match backend {
            Backend::Duckdb => MutationTarget::duckdb("otel_spans"),
            Backend::Clickhouse => MutationTarget::clickhouse("otel_spans_local", ""),
        };
        let delete = delete_traces(target, "p", &traces).unwrap();
        let enqueue = enqueue_raw_for_traces(backend, "p", &traces).unwrap();
        assert_eq!(delete.params(), enqueue.params());
        let predicate = delete
            .sql()
            .split(" WHERE ")
            .nth(1)
            .unwrap()
            .trim_end_matches(" SETTINGS mutations_sync = 2");
        assert!(enqueue.sql().contains("FROM otel_raw_traces"));
        assert!(
            enqueue.sql().ends_with(&format!("AND {predicate})")),
            "{} does not end with the delete's predicate {predicate}",
            enqueue.sql()
        );
        assert_eq!(enqueue.sql().matches('?').count(), enqueue.params().len());

        let spans = vec![
            ("t1".to_string(), "s1".to_string()),
            ("t1".to_string(), "s2".to_string()),
        ];
        assert_eq!(
            enqueue_raw_for_spans(backend, "p", &spans).unwrap(),
            enqueue_raw_for_traces(backend, "p", &["t1".to_string()]).unwrap()
        );
        assert!(enqueue_raw_for_traces(backend, "p", &[]).is_none());
        assert!(enqueue_raw_records(backend, "p", &[]).is_none());
        let by_id = enqueue_raw_records(backend, "p", &["r1".to_string()]).unwrap();
        assert_eq!(by_id.sql().matches('?').count(), by_id.params().len());
    }
}
