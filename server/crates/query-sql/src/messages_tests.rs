use super::*;
use chrono::{DateTime, Utc};
use sideseat_ports::types::ProjectId;

/// How many times a span winner as of a watermark binds it: DuckDB's condition names it twice
/// (`crate::winners`), ClickHouse's ranking once.
fn span_watermark_binds(backend: Backend) -> usize {
    match backend {
        Backend::Duckdb => 2,
        Backend::Clickhouse => 1,
    }
}

#[test]
fn message_selector_priority_and_watermark_bind_order_are_explicit() {
    let params = MessageQueryParams {
        project_id: ProjectId::from("tenant-'quoted"),
        span_id: Some("span-'quoted".to_string()),
        trace_id: Some("trace-'quoted".to_string()),
        session_id: Some("ignored-session".to_string()),
        trace_ids: Some(vec!["ignored-trace".to_string()]),
        ingested_before_us: Some(99),
        ..Default::default()
    };
    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let query = get_messages(&params, backend);
        assert_eq!(query.sql().matches('?').count(), query.params().len());
        let watermarks = span_watermark_binds(backend);
        assert!(
            query.params()[..watermarks]
                .iter()
                .all(|value| value == &QueryValue::Int64(99))
        );
        assert_eq!(
            query.params().get(watermarks),
            Some(&QueryValue::String("tenant-'quoted".to_string()))
        );
        assert!(query.sql().contains("span_id = ? AND trace_id = ?"));
        assert!(!query.sql().contains("ignored-session"));
        assert!(!query.sql().contains("ignored-trace"));
    }
}

#[test]
fn empty_trace_batch_is_fail_closed_and_project_feed_uses_total_cursor() {
    let empty = MessageQueryParams {
        project_id: ProjectId::from("p"),
        trace_ids: Some(Vec::new()),
        ..Default::default()
    };
    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let query = get_messages(&empty, backend);
        assert!(query.sql().contains("1 = 0"));
        assert!(query.sql().contains(MESSAGE_CONTENT_FILTER));

        let feed = get_project_messages(
            &FeedMessagesParams {
                project_id: ProjectId::from("tenant-'quoted"),
                limit: 17,
                cursor: Some((42, "span-'quoted".to_string(), "trace-'quoted".to_string())),
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
                ingested_before_us: Some(100),
            },
            backend,
        );
        assert_eq!(feed.sql().matches('?').count(), feed.params().len());
        // DuckDB finds the spans with log messages, picks the page and then joins their messages to it
        // (`duckdb_project_messages`): the log bound is bound twice.
        let (span_reads, log_reads) = match backend {
            Backend::Duckdb => (1, 2),
            Backend::Clickhouse => (1, 1),
        };
        let count = |wanted: &QueryValue| {
            feed.params()
                .iter()
                .filter(|value| *value == wanted)
                .count()
        };
        assert_eq!(
            count(&QueryValue::Int64(100)),
            span_reads * span_watermark_binds(backend) + log_reads
        );
        assert_eq!(count(&QueryValue::Int64(42)), span_reads);
        assert!(
            feed.sql()
                .contains("ORDER BY ingested_at_us DESC, span_id DESC, trace_id DESC")
        );
        assert!(feed.sql().ends_with("LIMIT 17"));
        assert!(!feed.sql().contains("tenant-'quoted"));
    }
}

#[test]
fn session_membership_uses_the_same_watermark_as_message_rows() {
    let params = MessageQueryParams {
        project_id: ProjectId::from("p"),
        session_id: Some("session-a".to_string()),
        ingested_before_us: Some(123),
        ..Default::default()
    };
    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let query = get_messages(&params, backend);
        assert_eq!(query.sql().matches('?').count(), query.params().len());
        assert_eq!(
            query
                .params()
                .iter()
                .filter(|value| matches!(value, QueryValue::Int64(123)))
                .count(),
            // Span rows, their session membership and the log aggregate's session scope are span
            // winner conditions; the log aggregate's own bound is one.
            3 * span_watermark_binds(backend) + 1,
            "span rows, their session membership, the log aggregate and its session scope must all \
             use one traversal instant"
        );
        assert!(query.sql().contains("canonical_session = ?"));
    }
}

fn every_selector() -> Vec<MessageQueryParams> {
    let project_id = ProjectId::from("p");
    vec![
        MessageQueryParams {
            project_id: project_id.clone(),
            span_id: Some("s".to_string()),
            trace_id: Some("t".to_string()),
            ingested_before_us: Some(7),
            ..Default::default()
        },
        MessageQueryParams {
            project_id: project_id.clone(),
            span_id: Some("s".to_string()),
            ..Default::default()
        },
        MessageQueryParams {
            project_id: project_id.clone(),
            session_id: Some("session".to_string()),
            ingested_before_us: Some(7),
            ..Default::default()
        },
        MessageQueryParams {
            project_id: project_id.clone(),
            trace_id: Some("t".to_string()),
            ..Default::default()
        },
        MessageQueryParams {
            project_id: project_id.clone(),
            trace_ids: Some(vec!["t1".to_string(), "t2".to_string()]),
            ingested_before_us: Some(7),
            ..Default::default()
        },
        MessageQueryParams {
            project_id,
            trace_ids: Some(Vec::new()),
            ..Default::default()
        },
    ]
}

/// One statement per read, every placeholder bound, and the log aggregate narrowed by the same
/// selector and bounded by the same watermark as the span rows.
#[test]
fn log_messages_join_in_the_same_statement_with_matching_binds() {
    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        for params in every_selector() {
            let query = get_messages(&params, backend);
            assert_eq!(
                query.sql().matches('?').count(),
                query.params().len(),
                "{backend:?} {params:?}"
            );
            assert_eq!(query.sql().matches("LEFT JOIN").count(), 1);
            assert!(query.sql().contains("FROM otel_logs"));
            let watermarks = query
                .params()
                .iter()
                .filter(|value| matches!(value, QueryValue::Int64(7)))
                .count();
            let span = span_watermark_binds(backend);
            match params.ingested_before_us {
                // The span source and the log aggregate, plus both copies of session membership.
                Some(_) if params.session_id.is_some() && params.span_id.is_none() => {
                    assert_eq!(watermarks, 3 * span + 1)
                }
                Some(_) => assert_eq!(watermarks, span + 1, "{backend:?} {params:?}"),
                None => assert_eq!(watermarks, 0),
            }
            let filtered = query.sql().contains(MESSAGE_CONTENT_FILTER);
            assert_eq!(
                filtered,
                params.span_id.is_none(),
                "the span view alone applies no content filter"
            );
        }
        let feed = get_project_messages(
            &FeedMessagesParams {
                project_id: ProjectId::from("p"),
                limit: 5,
                ingested_before_us: Some(7),
                ..Default::default()
            },
            backend,
        );
        assert_eq!(feed.sql().matches('?').count(), feed.params().len());
        assert_eq!(
            feed.params()
                .iter()
                .filter(|value| matches!(value, QueryValue::Int64(7)))
                .count(),
            match backend {
                // The log messages are found, to pick the page, and then joined to it.
                Backend::Duckdb => span_watermark_binds(backend) + 2,
                Backend::Clickhouse => span_watermark_binds(backend) + 1,
            },
            "a feed page and the log messages joined to it describe one instant"
        );
        let filter_at = feed.sql().find(MESSAGE_CONTENT_FILTER).expect("filtered");
        let limit_at = feed.sql().rfind("LIMIT 5").expect("limited");
        assert!(
            filter_at < limit_at && feed.sql().ends_with("LIMIT 5"),
            "the page is cut after the joined rows are filtered"
        );
    }
}

#[test]
fn the_content_filter_admits_a_span_whose_only_messages_are_in_logs() {
    assert!(MESSAGE_CONTENT_FILTER.contains("has_log_messages"));
    // And every read that filters projects it, on both backends; ClickHouse's feed from the carriers' keys.
    let feed = FeedMessagesParams {
        project_id: ProjectId::from("p"),
        limit: 5,
        ..Default::default()
    };
    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let trace = get_messages(
            &MessageQueryParams {
                project_id: ProjectId::from("p"),
                trace_id: Some("t".to_string()),
                ..Default::default()
            },
            backend,
        );
        for query in [trace, get_project_messages(&feed, backend)] {
            assert!(
                query.sql().contains("AS has_log_messages"),
                "{}",
                query.sql()
            );
            assert_eq!(query.sql().matches('?').count(), query.params().len());
        }
    }
    assert!(
        get_project_messages(&feed, Backend::Clickhouse)
            .sql()
            .contains("messages_bytes > 0"),
        "the feed page reads the carriers' keys, not their messages"
    );
}

#[test]
fn a_session_scopes_clickhouse_logs_with_a_global_subquery() {
    let params = MessageQueryParams {
        project_id: ProjectId::from("p"),
        session_id: Some("session".to_string()),
        ..Default::default()
    };
    assert!(
        get_messages(&params, Backend::Clickhouse)
            .sql()
            .contains("trace_id GLOBAL IN (")
    );
    assert!(
        !get_messages(&params, Backend::Duckdb)
            .sql()
            .contains("GLOBAL")
    );
}
