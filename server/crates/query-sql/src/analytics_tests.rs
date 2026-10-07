use super::*;
use chrono::{DateTime, Utc};
use sideseat_ports::filters::{Filter, NumberOp, OptionsOp, StringOp};
use sideseat_ports::types::ProjectId;

#[test]
fn span_point_read_declares_bind_order_once() {
    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let query = span_by_id().render(backend);
        assert_eq!(
            query.bindings(),
            &[Binding::ProjectId, Binding::TraceId, Binding::SpanId]
        );
        assert_eq!(query.sql().matches('?').count(), 3);
    }
}

#[test]
fn span_point_read_lowers_deduplication_as_a_capability() {
    let duckdb = span_by_id().render(Backend::Duckdb);
    assert!(duckdb.sql().contains("QUALIFY ROW_NUMBER()"));
    assert!(duckdb.sql().contains("rowid DESC"));
    assert!(!duckdb.sql().contains(" FINAL"));

    let clickhouse = span_by_id().render(Backend::Clickhouse);
    assert!(clickhouse.sql().contains("FROM otel_spans FINAL"));
    assert!(!clickhouse.sql().contains("QUALIFY"));
}

#[test]
fn registry_is_derived_from_real_typed_operations() {
    assert_eq!(
        MIGRATED_OPERATIONS,
        &[
            span_by_id().operation(),
            QueryOperation::ListSpans,
            QueryOperation::ListTraces,
            QueryOperation::DeleteTraces,
            QueryOperation::DeleteSpans,
            QueryOperation::DeleteProjectData,
            QueryOperation::UpsertSpans,
            QueryOperation::UpsertMetrics,
            QueryOperation::UpsertLogs,
            QueryOperation::ListMetrics,
            QueryOperation::GetMetric,
            QueryOperation::AggregateMetrics,
            QueryOperation::GetMetricFilterOptions,
            QueryOperation::ListLogs,
            QueryOperation::GetLog,
            QueryOperation::GetLogFilterOptions,
            QueryOperation::GetTraceSessionPairs,
            QueryOperation::GetSessionIdsForTraces,
            QueryOperation::GetTraceIdsForSessions,
            QueryOperation::GetSpanCountsBulk,
            QueryOperation::TracesWithoutSpans,
            QueryOperation::FileReferenceFieldsForTraces,
            QueryOperation::CountProjectRows,
            QueryOperation::AnalyticsProjectIds,
            QueryOperation::CountSpansByProject,
            QueryOperation::GetSpansForTrace,
            QueryOperation::GetTrace,
            QueryOperation::GetTracesForSession,
            QueryOperation::GetSession,
            QueryOperation::ListSessions,
            QueryOperation::GetFeedSpans,
            QueryOperation::GetTraceFilterOptions,
            QueryOperation::GetTraceTagsOptions,
            QueryOperation::GetSpanFilterOptions,
            QueryOperation::GetSessionFilterOptions,
            QueryOperation::GetMessages,
            QueryOperation::GetProjectMessages,
            QueryOperation::GetProjectStats,
            QueryOperation::MaxIngestedAtUs,
            QueryOperation::EnforceRetention,
            QueryOperation::DeleteSessions,
            QueryOperation::Search,
            QueryOperation::RawReconciliation,
        ]
    );
    assert_eq!(
        MIGRATED_OPERATIONS
            .iter()
            .map(|operation| operation.name())
            .collect::<Vec<_>>(),
        vec![
            "get_span",
            "list_spans",
            "list_traces",
            "delete_traces",
            "delete_spans",
            "delete_project_data",
            "upsert_spans",
            "upsert_metrics",
            "upsert_logs",
            "list_metrics",
            "get_metric",
            "aggregate_metrics",
            "get_metric_filter_options",
            "list_logs",
            "get_log",
            "get_log_filter_options",
            "get_trace_session_pairs",
            "get_session_ids_for_traces",
            "get_trace_ids_for_sessions",
            "get_span_counts_bulk",
            "traces_without_spans",
            "file_reference_fields_for_traces",
            "count_project_rows",
            "analytics_project_ids",
            "count_spans_by_project",
            "get_spans_for_trace",
            "get_trace",
            "get_traces_for_session",
            "get_session",
            "list_sessions",
            "get_feed_spans",
            "get_trace_filter_options",
            "get_trace_tags_options",
            "get_span_filter_options",
            "get_session_filter_options",
            "get_messages",
            "get_project_messages",
            "get_project_stats",
            "max_ingested_at_us",
            "enforce_retention",
            "delete_sessions",
            "search",
            "raw_reconciliation",
        ]
    );
}

#[test]
fn session_page_keeps_values_bound_and_entity_negations_canonical() {
    let params = ListSessionsParams {
        project_id: ProjectId::from("tenant-'quoted"),
        page: 2,
        limit: 17,
        user_id: Some("user-'quoted".to_string()),
        environment: Some(vec!["prod".to_string(), "stage".to_string()]),
        from_timestamp: Some(
            DateTime::parse_from_rfc3339("2026-09-20T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        ),
        to_timestamp: Some(
            DateTime::parse_from_rfc3339("2026-09-21T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        ),
        filters: vec![
            Filter::StringOptions {
                column: "session_id".to_string(),
                operator: OptionsOp::NoneOf,
                value: vec!["session-'forbidden".to_string()],
            },
            Filter::StringOptions {
                column: "gen_ai_request_model".to_string(),
                operator: OptionsOp::NoneOf,
                value: vec!["model-'forbidden".to_string()],
            },
            Filter::Number {
                column: "gen_ai_cost_total".to_string(),
                operator: NumberOp::Gt,
                value: 1.25,
            },
        ],
        ..Default::default()
    };

    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let page = list_sessions(&params, backend);
        for query in [&page.count, &page.rows] {
            assert_eq!(query.sql().matches('?').count(), query.params().len());
            assert!(!query.sql().contains("tenant-'quoted"));
            assert!(!query.sql().contains("user-'quoted"));
            assert!(!query.sql().contains("forbidden"));
        }
        assert!(page.rows.sql().contains("sp.trace_id NOT IN"));
        assert!(page.rows.sql().contains("sp.session_id NOT IN"));
        assert!(page.rows.sql().contains("n.gen_ai_request_model IN (?)"));
        assert!(page.rows.sql().contains("LIMIT 17 OFFSET 17"));
    }

    let duckdb = list_sessions(&params, Backend::Duckdb);
    assert!(
        duckdb
            .rows
            .params()
            .iter()
            .any(|value| matches!(value, QueryValue::String(value) if value == "1.25"))
    );
    let clickhouse = list_sessions(&params, Backend::Clickhouse);
    assert!(
        clickhouse
            .rows
            .params()
            .iter()
            .any(|value| matches!(value, QueryValue::Float64(value) if *value == 1.25))
    );
    assert!(
        clickhouse
            .rows
            .params()
            .iter()
            .any(|value| matches!(value, QueryValue::Int64(_)))
    );
}

#[test]
fn feed_page_puts_watermark_and_cursor_values_in_total_key_order() {
    let params = FeedSpansParams {
        project_id: ProjectId::from("tenant-'quoted"),
        limit: 13,
        cursor: Some((77, "span-'quoted".to_string(), "trace-'quoted".to_string())),
        start_time: Some(
            DateTime::parse_from_rfc3339("2026-09-20T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        ),
        end_time: Some(
            DateTime::parse_from_rfc3339("2026-09-21T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        ),
        is_observation: Some(true),
        ingested_before_us: Some(999),
    };

    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let query = feed_spans(&params, backend);
        assert_eq!(query.sql().matches('?').count(), query.params().len());
        assert!(!query.sql().contains("tenant-'quoted"));
        assert!(!query.sql().contains("span-'quoted"));
        assert!(!query.sql().contains("trace-'quoted"));
        assert_eq!(query.params().first(), Some(&QueryValue::Int64(999)));
        assert_eq!(
            query.params().get(1),
            Some(&QueryValue::String("tenant-'quoted".to_string()))
        );
        assert_eq!(query.params().get(2), Some(&QueryValue::Int64(77)));
        assert!(
            query
                .sql()
                .contains("ORDER BY ingested_at DESC, span_id DESC, trace_id DESC")
        );
        assert!(query.sql().ends_with("LIMIT 13"));
        assert!(query.sql().contains("AS ingested_at_us"));
    }

    let duckdb = feed_spans(&params, Backend::Duckdb);
    assert!(duckdb.sql().contains("QUALIFY ROW_NUMBER()"));
    assert!(duckdb.sql().contains("rowid DESC"));
    assert!(matches!(
        duckdb.params().get(5),
        Some(QueryValue::String(value)) if value.starts_with("2026-09-20")
    ));

    let clickhouse = feed_spans(&params, Backend::Clickhouse);
    assert!(
        clickhouse
            .sql()
            .contains("LIMIT 1 BY project_id, trace_id, span_id")
    );
    assert!(matches!(
        clickhouse.params().get(5),
        Some(QueryValue::Int64(_))
    ));
}

#[test]
fn filter_option_queries_validate_columns_and_read_winning_entities() {
    let from = DateTime::parse_from_rfc3339("2026-09-20T10:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let to = DateTime::parse_from_rfc3339("2026-09-21T10:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let requested = vec![
        "environment".to_string(),
        "session_id".to_string(),
        "trace_name".to_string(),
        "environment; SELECT secret".to_string(),
    ];

    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let trace = trace_filter_options(
            "tenant-'quoted",
            &requested,
            Some(&from),
            Some(&to),
            backend,
        );
        assert_eq!(trace.len(), 3);
        let span = span_filter_options(
            "tenant-'quoted",
            &["status_code".to_string(), "bad column".to_string()],
            Some(&from),
            Some(&to),
            true,
            backend,
        );
        assert_eq!(span.len(), 1);
        let session = session_filter_options(
            "tenant-'quoted",
            &["environment".to_string(), "bad column".to_string()],
            Some(&from),
            Some(&to),
            backend,
        );
        assert_eq!(session.len(), 1);
        let tags = trace_tag_options("tenant-'quoted", Some(&from), Some(&to), backend);

        for query in trace
            .iter()
            .map(|option| &option.query)
            .chain(span.iter().map(|option| &option.query))
            .chain(session.iter().map(|option| &option.query))
            .chain(std::iter::once(&tags))
        {
            assert_eq!(query.sql().matches('?').count(), query.params().len());
            assert!(!query.sql().contains("tenant-'quoted"));
            assert!(!query.sql().contains("bad column"));
            assert!(!query.sql().contains("SELECT secret"));
            match backend {
                Backend::Duckdb => {
                    assert!(query.sql().contains("QUALIFY ROW_NUMBER()"));
                    assert!(query.sql().contains("rowid DESC"));
                    assert!(matches!(
                        query.params().get(1),
                        Some(QueryValue::String(value)) if value.starts_with("2026-09-20")
                    ));
                }
                Backend::Clickhouse => {
                    assert!(query.sql().contains("FINAL"));
                    assert!(
                        query.sql().contains("toNullable("),
                        "filter-option values must match the adapter's Nullable(String) row"
                    );
                    assert!(
                        !query.sql().contains("FINAL s")
                            && !query.sql().contains("FINAL sp")
                            && !query.sql().contains("FINAL g"),
                        "ClickHouse requires table aliases before FINAL; winner relations use subqueries"
                    );
                    assert!(matches!(query.params().get(1), Some(QueryValue::Int64(_))));
                }
            }
        }

        assert!(session[0].query.sql().contains("COUNT(DISTINCT cts."));
        assert!(!session[0].query.sql().contains("s.session_id IS NOT NULL"));
        assert!(
            span[0]
                .query
                .sql()
                .contains("gen_ai_request_model IS NOT NULL")
        );
    }
}

#[test]
fn filtered_span_page_keeps_values_out_of_sql_and_in_driver_types() {
    let params = ListSpansParams {
        project_id: ProjectId::from("tenant-with-'quotes"),
        trace_id: Some("trace-value".to_string()),
        environment: Some(vec!["prod".to_string(), "stage".to_string()]),
        filters: vec![
            Filter::String {
                column: "span_name".to_string(),
                operator: StringOp::Contains,
                value: "50%_'quoted".to_string(),
            },
            Filter::Number {
                column: "gen_ai_cost_total".to_string(),
                operator: NumberOp::Gt,
                value: 1.25,
            },
        ],
        limit: 25,
        ..Default::default()
    };

    let duckdb = list_spans(&params, Backend::Duckdb);
    let clickhouse = list_spans(&params, Backend::Clickhouse);

    for page in [&duckdb, &clickhouse] {
        assert!(!page.rows.sql().contains("tenant-with-"));
        assert!(!page.rows.sql().contains("trace-value"));
        assert!(!page.rows.sql().contains("quoted"));
        assert_eq!(page.count.params(), page.rows.params());
        assert!(page.rows.sql().ends_with("LIMIT 25 OFFSET 0"));
    }
    assert!(matches!(
        duckdb.rows.params().last(),
        Some(QueryValue::String(value)) if value == "1.25"
    ));
    assert!(matches!(
        clickhouse.rows.params().last(),
        Some(QueryValue::Float64(value)) if *value == 1.25
    ));
    assert!(duckdb.rows.sql().contains("ESCAPE '\\'"));
    assert!(
        clickhouse
            .rows
            .sql()
            .contains("toFloat64(gen_ai_cost_total) > ?")
    );
}

#[test]
fn session_parameter_uses_the_canonical_trace_membership_relation() {
    let params = ListSpansParams {
        project_id: ProjectId::from("p"),
        session_id: Some("session-a".to_string()),
        limit: 10,
        ..Default::default()
    };

    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let page = list_spans(&params, backend);
        assert!(page.rows.sql().contains("arg_") || page.rows.sql().contains("argMin"));
        assert!(page.rows.sql().contains("canonical_session = ?"));
        assert_eq!(page.rows.params().len(), 5);
        assert_eq!(
            page.rows.params(),
            &[
                QueryValue::String("p".to_string()),
                QueryValue::String("p".to_string()),
                QueryValue::String("p".to_string()),
                QueryValue::String("session-a".to_string()),
                QueryValue::String("session-a".to_string()),
            ]
        );
    }
}

#[test]
fn trace_page_keeps_every_value_bound_in_statement_order() {
    let params = ListTracesParams {
        project_id: ProjectId::from("tenant-with-'quotes"),
        user_id: Some("user-'quoted".to_string()),
        environment: Some(vec!["prod".to_string(), "stage".to_string()]),
        from_timestamp: Some(
            "2026-01-02T03:04:05Z"
                .parse::<DateTime<Utc>>()
                .expect("valid timestamp"),
        ),
        to_timestamp: Some(
            "2026-02-03T04:05:06Z"
                .parse::<DateTime<Utc>>()
                .expect("valid timestamp"),
        ),
        filters: vec![
            Filter::String {
                column: "trace_name".to_string(),
                operator: StringOp::Contains,
                value: "50%_'quoted".to_string(),
            },
            Filter::Number {
                column: "total_cost".to_string(),
                operator: NumberOp::Gt,
                value: 1.25,
            },
        ],
        page: 2,
        limit: 25,
        ..Default::default()
    };

    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let page = list_traces(&params, backend);
        for query in [&page.count, &page.rows] {
            assert_eq!(query.sql().matches('?').count(), query.params().len());
            assert!(!query.sql().contains("tenant-with-"));
            assert!(!query.sql().contains("user-'quoted"));
            assert!(!query.sql().contains("50%_'quoted"));
            assert!(!query.sql().contains("1.25"));
        }
        assert!(page.rows.sql().contains("LIMIT 25 OFFSET 25"));
    }
}

#[test]
fn trace_aggregate_filters_preserve_backend_driver_types() {
    let params = ListTracesParams {
        project_id: ProjectId::from("p"),
        filters: vec![Filter::Number {
            column: "total_tokens".to_string(),
            operator: NumberOp::Gte,
            value: 42.5,
        }],
        limit: 10,
        ..Default::default()
    };

    let duckdb = list_traces(&params, Backend::Duckdb);
    let clickhouse = list_traces(&params, Backend::Clickhouse);

    assert!(
        duckdb
            .rows
            .sql()
            .contains("COALESCE(MAX(gtf.total_tokens), 0)")
    );
    assert!(
        duckdb
            .rows
            .params()
            .iter()
            .any(|value| matches!(value, QueryValue::String(value) if value == "42.5"))
    );
    assert!(
        clickhouse
            .rows
            .sql()
            .contains("COALESCE(max(gtf.total_tokens), 0)")
    );
    assert!(
        clickhouse
            .rows
            .params()
            .iter()
            .any(|value| matches!(value, QueryValue::Float64(value) if *value == 42.5))
    );
}

#[test]
fn negated_trace_session_filter_uses_canonical_entity_complement() {
    let params = ListTracesParams {
        project_id: ProjectId::from("p"),
        filters: vec![Filter::StringOptions {
            column: "session_id".to_string(),
            operator: OptionsOp::NoneOf,
            value: vec!["session-a".to_string(), "session-b".to_string()],
        }],
        limit: 10,
        ..Default::default()
    };

    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let page = list_traces(&params, backend);
        for query in [&page.count, &page.rows] {
            assert!(query.sql().contains("NOT IN"));
            assert!(query.sql().contains("arg_min") || query.sql().contains("argMin"));
            assert!(!query.sql().contains("session-a"));
            assert_eq!(query.sql().matches('?').count(), query.params().len());
        }
    }
}

#[test]
fn membership_reads_put_the_watermark_before_tenant_and_id_values() {
    let trace_ids = vec!["trace-'one".to_string(), "trace-two".to_string()];
    let session_ids = vec!["session-'one".to_string(), "session-two".to_string()];

    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let pairs =
            trace_session_pairs("tenant-'quoted", &trace_ids, Some(123), backend).expect("pairs");
        let sessions = session_ids_for_traces("tenant-'quoted", &trace_ids, Some(123), backend)
            .expect("sessions");
        let traces = trace_ids_for_sessions("tenant-'quoted", &session_ids, Some(123), backend)
            .expect("traces");
        for query in [&pairs, &sessions, &traces] {
            assert_eq!(query.sql().matches('?').count(), query.params().len());
            assert!(!query.sql().contains("tenant-'quoted"));
            assert!(!query.sql().contains("trace-'one"));
            assert!(!query.sql().contains("session-'one"));
        }
        match backend {
            Backend::Duckdb => {
                assert_eq!(
                    pairs.params().first(),
                    Some(&QueryValue::String("tenant-'quoted".to_string()))
                );
                assert_eq!(
                    pairs.params().get(1),
                    Some(&QueryValue::String("123".to_string()))
                );
                assert!(pairs.sql().contains("WHERE project_id = ? AND EPOCH_US"));
                for query in [&sessions, &traces] {
                    assert_eq!(
                        query.params().first(),
                        Some(&QueryValue::String("123".to_string()))
                    );
                }
            }
            Backend::Clickhouse => {
                for query in [&pairs, &sessions, &traces] {
                    assert_eq!(query.params().first(), Some(&QueryValue::Int64(123)));
                }
            }
        }
        for query in [&sessions, &traces] {
            assert_eq!(
                query.params().get(1),
                Some(&QueryValue::String("tenant-'quoted".to_string()))
            );
        }
    }
}

#[test]
fn membership_reads_use_canonical_earliest_span_semantics() {
    let trace_ids = vec!["trace".to_string()];
    let sessions = vec!["session".to_string()];
    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let pairs = trace_session_pairs("p", &trace_ids, None, backend).expect("trace pairs");
        let reverse =
            trace_ids_for_sessions("p", &sessions, None, backend).expect("session traces");
        for query in [&pairs, &reverse] {
            assert!(query.sql().contains("timestamp_start, span_id"));
            assert!(
                query.sql().contains("canonical_session") || query.sql().contains("AS session")
            );
        }
        match backend {
            Backend::Duckdb => assert!(reverse.sql().contains("ROW_NUMBER()")),
            Backend::Clickhouse => assert!(reverse.sql().contains("FINAL")),
        }
    }
}

#[test]
fn bulk_span_counts_use_winning_rows_and_tuple_bindings() {
    let spans = vec![
        ("trace-'a".to_string(), "span-a".to_string()),
        ("trace-b".to_string(), "span-b".to_string()),
    ];
    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let query = span_counts_bulk("tenant-'quoted", &spans, backend).expect("counts");
        assert_eq!(query.sql().matches('?').count(), query.params().len());
        assert_eq!(query.params().len(), 5);
        assert!(!query.sql().contains("tenant-'quoted"));
        assert!(!query.sql().contains("trace-'a"));
        // The counts are columns, so the only dialect difference left is how the winning row is chosen.
        assert!(
            query
                .sql()
                .contains("SELECT trace_id, span_id, event_count, link_count")
        );
        match backend {
            Backend::Duckdb => assert!(query.sql().contains("QUALIFY ROW_NUMBER()")),
            Backend::Clickhouse => assert!(query.sql().contains("FROM otel_spans FINAL")),
        }
    }
}

#[test]
fn cleanup_trace_reads_share_winner_and_tenant_scoping() {
    let traces = vec!["trace-'a".to_string(), "trace-b".to_string()];
    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let alive = surviving_trace_ids("tenant-'quoted", &traces, backend).expect("alive");
        let fields = file_reference_fields("tenant-'quoted", &traces, backend).expect("fields");
        for query in [&alive, &fields] {
            assert_eq!(query.sql().matches('?').count(), query.params().len());
            assert!(query.sql().contains("project_id = ?"));
            assert!(!query.sql().contains("tenant-'quoted"));
            assert!(!query.sql().contains("trace-'a"));
        }
        match backend {
            Backend::Duckdb => assert!(alive.sql().contains("QUALIFY ROW_NUMBER()")),
            Backend::Clickhouse => {
                assert!(alive.sql().contains("FROM otel_spans FINAL"));
                assert!(fields.sql().contains("coalesce(messages, '')"));
            }
        }
    }
}

#[test]
fn maintenance_count_plans_are_explicit_and_parameterized() {
    let duckdb = project_row_count("tenant-'quoted", Backend::Duckdb, None);
    assert!(duckdb.metrics_table_exists.is_none());
    let clickhouse = project_row_count(
        "tenant-'quoted",
        Backend::Clickhouse,
        Some("otel_metrics_local"),
    );
    assert!(clickhouse.metrics_table_exists.is_some());
    for query in [
        &duckdb.spans,
        &duckdb.metrics,
        &clickhouse.spans,
        clickhouse
            .metrics_table_exists
            .as_ref()
            .expect("table existence"),
        &clickhouse.metrics,
    ] {
        assert_eq!(query.sql().matches('?').count(), query.params().len());
        assert!(!query.sql().contains("tenant-'quoted"));
    }

    let projects = vec!["tenant-'one".to_string(), "tenant-two".to_string()];
    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let query = span_counts_by_project(&projects, backend).expect("project counts");
        assert_eq!(query.sql().matches('?').count(), 2);
        assert_eq!(query.params().len(), 2);
        assert!(!query.sql().contains("tenant-'one"));
    }
    assert!(span_counts_by_project(&[], Backend::Duckdb).is_none());
}

#[test]
fn trace_span_detail_read_reuses_projection_and_winner_capabilities() {
    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let query = spans_for_trace("tenant-'quoted", "trace-'quoted", 73, backend);
        assert_eq!(query.sql().matches('?').count(), 3);
        assert_eq!(query.params().len(), 3);
        assert_eq!(query.params()[2], QueryValue::Int64(73));
        assert!(!query.sql().contains("tenant-'quoted"));
        assert!(!query.sql().contains("trace-'quoted"));
        assert!(query.sql().contains("scope_name"));
        assert!(query.sql().contains("ORDER BY timestamp_start"));
        match backend {
            Backend::Duckdb => assert!(query.sql().contains("QUALIFY ROW_NUMBER()")),
            Backend::Clickhouse => assert!(query.sql().contains("FROM otel_spans FINAL")),
        }

        let capped = spans_for_trace("tenant", "trace", usize::MAX, backend);
        assert_eq!(
            capped.params()[2],
            QueryValue::Int64(i64::from(QUERY_MAX_SPANS_PER_TRACE))
        );
    }
}

#[test]
fn trace_point_aggregate_reuses_list_projection_and_token_dedup() {
    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let query = trace_by_id("tenant-'quoted", "trace-'quoted", backend);
        assert_eq!(query.sql().matches('?').count(), query.params().len());
        assert!(!query.sql().contains("tenant-'quoted"));
        assert!(!query.sql().contains("trace-'quoted"));
        assert!(query.sql().contains("total_tokens"));
        assert!(query.sql().contains("input_preview"));
        match backend {
            Backend::Duckdb => {
                assert_eq!(query.params().len(), 4);
                assert!(query.sql().contains("QUALIFY ROW_NUMBER()"));
                assert!(query.sql().contains("timestamp_start, s.span_id"));
                assert!(query.sql().contains("NOT EXISTS"));
            }
            Backend::Clickhouse => {
                assert_eq!(query.params().len(), 6);
                assert!(query.sql().contains("timestamp_start, s.span_id"));
                assert!(query.sql().contains("dedup_lookup AS"));
                assert!(query.sql().contains("NOT IN"));
            }
        }
    }
}

#[test]
fn session_trace_aggregate_composes_canonical_membership_and_shared_projection() {
    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let query = traces_for_session("tenant-'quoted", "session-'quoted", backend);
        assert_eq!(query.sql().matches('?').count(), query.params().len());
        assert!(!query.sql().contains("tenant-'quoted"));
        assert!(!query.sql().contains("session-'quoted"));
        assert!(query.sql().contains("canonical_session = ?"));
        assert!(query.sql().contains("input_preview"));
        assert!(query.sql().contains("total_tokens"));
        assert!(query.sql().contains("ORDER BY"));
        match backend {
            Backend::Duckdb => assert_eq!(query.params().len(), 6),
            Backend::Clickhouse => {
                assert_eq!(query.params().len(), 7);
                assert!(query.sql().contains("dedup_lookup AS"));
            }
        }
    }
}

#[test]
fn session_point_aggregate_uses_canonical_membership_and_shared_totals() {
    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let query = session_by_id("tenant-'quoted", "session-'quoted", backend);
        assert_eq!(query.sql().matches('?').count(), query.params().len());
        assert!(!query.sql().contains("tenant-'quoted"));
        assert!(!query.sql().contains("session-'quoted"));
        assert!(query.sql().contains("canonical_session = ?"));
        assert!(query.sql().contains("gen_totals_by_trace"));
        assert!(query.sql().contains("session_totals"));
        assert!(query.sql().contains("COUNT(DISTINCT s.trace_id)"));
        match backend {
            Backend::Duckdb => assert_eq!(query.params().len(), 7),
            Backend::Clickhouse => {
                assert_eq!(query.params().len(), 8);
                assert!(query.sql().contains("dedup_lookup AS"));
            }
        }
    }
}

#[test]
fn pressure_candidates_are_winning_held_aware_bounded_and_parameterized() {
    let now = chrono::DateTime::from_timestamp(1_795_000_000, 0).unwrap();
    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let query = oldest_reclaimable_spans("tenant-'quoted", backend, 10_000, now, 100);
        assert_eq!(query.sql().matches('?').count(), query.params().len());
        assert!(!query.sql().contains("tenant-'quoted"));
        assert!(query.sql().contains("hold_until IS NULL OR hold_until < ?"));
        assert!(query.sql().contains("bytes_before < ?"));
        assert!(query.sql().contains("pressure_rank <= ?"));
        match backend {
            Backend::Duckdb => assert!(query.sql().contains("QUALIFY ROW_NUMBER()")),
            Backend::Clickhouse => assert!(query.sql().contains("otel_spans FINAL")),
        }
    }
}
