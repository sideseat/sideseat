//! The lookups a write and its confirmation make read the rows they name, not the table.
//!
//! Each runs over a store holding many more rows than it names, with DuckDB's profiler on, and the rows its
//! statement scanned are held to a small multiple of the keys. A lookup whose key stops being its scan's only
//! filter, a dropped index, or a list longer than DuckDB keeps to the index for, reads the store instead, and
//! fails here. The term delete a correction makes is held to the row groups its revision's terms went to.

use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use sideseat_core::storage::AppStorage;
use sideseat_ports::traits::{
    EntityQuery, LogStore, MetricStore, RawStore, SpanStore, SurvivorReferences,
};
use sideseat_ports::types::{NormalizedSpan, ProjectId, RawOrigin, RawRecordRow, StagedSignal};

use super::repositories::keyed;
use super::{DuckdbRepository, DuckdbService, TestClock};

const PROJECT: &str = "p";
const TRACES: usize = 3_000;
const SPANS_PER_TRACE: usize = 10;
/// Logs and datapoints each: three of DuckDB's row groups.
const LOGS_AND_POINTS: usize = 3 * 122_880;
const START_US: i64 = 1_700_000_000_000_000;
/// One DuckDB row group: what a lookup bounded by instant reads when its instants lie in one.
const ROW_GROUP: u64 = 122_880;
/// A log or datapoint in the middle row group.
const MIDDLE: usize = 200_000;

fn at(i: usize) -> chrono::DateTime<Utc> {
    chrono::DateTime::from_timestamp_micros(START_US + i as i64 * 1_000_000).expect("instant")
}

fn trace(t: usize) -> String {
    format!("{t:032x}")
}

fn span(t: usize, s: usize) -> String {
    format!("{:016x}", t * SPANS_PER_TRACE + s)
}

fn raw_id(t: usize) -> String {
    format!("raw-{t:06}")
}

fn rows_scanned(profile: &std::path::Path) -> u64 {
    fn walk(node: &serde_json::Value, total: &mut u64) {
        let name = node["operator_name"]
            .as_str()
            .or(node["operator_type"].as_str())
            .unwrap_or_default();
        if name.contains("SCAN") && !name.contains("CTE") && !name.contains("COLUMN_DATA") {
            *total += node["operator_rows_scanned"].as_u64().unwrap_or(0);
        }
        for child in node["children"].as_array().into_iter().flatten() {
            walk(child, total);
        }
    }
    let text = std::fs::read_to_string(profile).expect("a profiled statement");
    let mut total = 0;
    walk(
        &serde_json::from_str::<serde_json::Value>(&text).expect("profile json"),
        &mut total,
    );
    total
}

struct Store {
    _temp: tempfile::TempDir,
    service: Arc<DuckdbService>,
    repo: DuckdbRepository,
    profile: std::path::PathBuf,
}

async fn store() -> Store {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let storage = AppStorage::init_for_test(temp.path().to_path_buf());
    std::fs::create_dir_all(storage.subdir(sideseat_core::storage::DataSubdir::Duckdb))
        .expect("duckdb dir");
    let service = Arc::new(
        DuckdbService::init(&storage, Arc::new(TestClock))
            .await
            .expect("duckdb"),
    );
    let repo = DuckdbRepository(Arc::clone(&service));
    let start = Utc.timestamp_opt(1_700_000_000, 0).single().expect("start");
    let spans: Vec<NormalizedSpan> = (0..TRACES)
        .flat_map(|t| {
            (0..SPANS_PER_TRACE).map(move |s| NormalizedSpan {
                project_id: Some(PROJECT.into()),
                trace_id: trace(t),
                span_id: span(t, s),
                content_digest: format!("digest-{t}-{s}"),
                span_name: "step".into(),
                timestamp_start: start + TimeDelta::seconds(t as i64),
                raw_id: Some(raw_id(t)),
                ..Default::default()
            })
        })
        .collect();
    for batch in spans.chunks(10_000) {
        repo.insert_spans(batch.to_vec()).await.expect("spans");
    }
    let records: Vec<RawRecordRow> = (0..TRACES)
        .map(|t| RawRecordRow {
            project_id: ProjectId::from(PROJECT),
            raw_id: raw_id(t),
            signal: StagedSignal::Traces,
            received_at: start,
            origin: RawOrigin::Received,
            version: 1,
            signal_until: start,
            hold_until: None,
            trace_ids: vec![trace(t)],
            record: b"record".to_vec(),
        })
        .collect();
    repo.insert_raw_records(&records).await.expect("records");
    // Logs and datapoints over three row groups, appended in the order of their instants, as telemetry
    // arrives: a lookup bounded by instant must read one row group, not three.
    service
        .write(|conn| {
            conn.execute_batch(&format!(
            "INSERT INTO otel_logs (project_id, log_digest, ordinal, \"timestamp\", severity_number, \
             dropped_attributes_count, flags, ingested_at) \
             SELECT '{PROJECT}', 'log-' || lpad(i::VARCHAR, 8, '0'), 0, make_timestamp({START_US} + i * 1000000), \
             9, 0, 0, make_timestamp({START_US}) FROM range({LOGS_AND_POINTS}) r(i) ORDER BY i; \
             INSERT INTO otel_metrics (project_id, metric_name, metric_type, \"timestamp\", datapoint_id, \
             content_digest, ingested_at) \
             SELECT '{PROJECT}', 'cpu', 'gauge', make_timestamp({START_US} + i * 1000000), \
             'dp-' || lpad(i::VARCHAR, 8, '0'), 'metric-digest-' || i::VARCHAR, make_timestamp({START_US}) \
             FROM range({LOGS_AND_POINTS}) r(i) ORDER BY i; CHECKPOINT;"
            ))
            .map_err(Into::into)
        })
        .expect("logs and datapoints");
    let profile = temp.path().join("profile.json");
    service
        .conn()
        .execute_batch(&format!(
            "PRAGMA enable_profiling = 'json'; PRAGMA profiling_output = '{}';",
            profile.display()
        ))
        .expect("profiling");
    Store {
        _temp: temp,
        service,
        repo,
        profile,
    }
}

fn identities(count: usize) -> Vec<(String, String, String)> {
    (0..count)
        .map(|n| {
            let (t, s) = (n % TRACES, (n / TRACES) % SPANS_PER_TRACE);
            (trace(t), span(t, s), format!("digest-{t}-{s}"))
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ingest_lookups_read_the_rows_they_name() {
    let s = store().await;
    let project = ProjectId::from(PROJECT);
    let total_spans = (TRACES * SPANS_PER_TRACE) as u64;
    let five = identities(5);
    let pairs: Vec<(String, String)> = five
        .iter()
        .map(|(trace, span, _)| (trace.clone(), span.clone()))
        .collect();
    let bound = |keys: usize| (keys * 2) as u64;

    macro_rules! held {
        ($name:expr, $keys:expr, $call:expr) => {{
            $call.await.expect($name);
            let scanned = rows_scanned(&s.profile);
            assert!(
                scanned <= bound($keys),
                "{}: scanned {scanned} of {total_spans} rows for {} keys",
                $name,
                $keys
            );
        }};
    }

    held!(
        "spans match content",
        5,
        s.repo.spans_match_content(&project, &five)
    );
    held!(
        "spans with matching content",
        5,
        s.repo.spans_with_matching_content(&project, &five)
    );
    held!("span raw ids", 5, s.repo.span_raw_ids(&project, &pairs));
    held!(
        "raw records",
        5,
        s.repo
            .get_raw_records(&project, &(0..5).map(raw_id).collect::<Vec<_>>())
    );
    // Bounded by instant: the row group that holds the records, of three.
    let logs: Vec<(String, u32, Option<chrono::DateTime<Utc>>)> = (MIDDLE..MIDDLE + 5)
        .map(|n| (format!("log-{n:08}"), 0, Some(at(n))))
        .collect();
    assert!(
        s.repo
            .logs_match_content(&project, &logs)
            .await
            .expect("logs")
    );
    let scanned = rows_scanned(&s.profile);
    assert!(scanned <= ROW_GROUP, "logs match content scanned {scanned}");
    let points: Vec<(String, String, chrono::DateTime<Utc>)> = (MIDDLE..MIDDLE + 5)
        .map(|n| (format!("dp-{n:08}"), format!("metric-digest-{n}"), at(n)))
        .collect();
    assert!(
        s.repo
            .metrics_match_content(&project, &points)
            .await
            .expect("metrics")
    );
    let scanned = rows_scanned(&s.profile);
    assert!(
        scanned <= ROW_GROUP,
        "metrics match content scanned {scanned}"
    );
    held!(
        "file reference fields of a trace",
        SPANS_PER_TRACE,
        s.repo
            .file_reference_fields_for_traces(&project, &[trace(7)])
    );
    held!(
        "traces without spans",
        SPANS_PER_TRACE,
        s.repo.traces_without_spans(&project, &[trace(7)])
    );
    held!(
        "survivor raw records",
        1,
        s.repo.survivor_raw_records(&project, &[trace(7)])
    );

    // Longer than DuckDB keeps to the index for: read in chunks, and the last chunk is held like any other.
    let many = identities(3_000);
    held!(
        "a long list of identities",
        3_000 % sideseat_query_sql::keyed::KEYED_CHUNK,
        s.repo.spans_with_matching_content(&project, &many)
    );

    // The lookups the writes make.
    {
        let conn = s.service.conn();
        let wanted: Vec<keyed::SpanIdentity> = five
            .iter()
            .map(|(trace, span, _)| (PROJECT.to_string(), trace.clone(), span.clone()))
            .collect();
        assert_eq!(
            keyed::span_revisions(&conn, &wanted, i64::MIN)
                .expect("revisions")
                .len(),
            5
        );
        let scanned = rows_scanned(&s.profile);
        assert!(scanned <= bound(5), "span revisions scanned {scanned}");

        // Records in the first and the last of three row groups read those two, not the one between them.
        let far = [10, MIDDLE + 100_000];
        let logs: Vec<(keyed::LogIdentity, Option<i64>)> = far
            .iter()
            .map(|n| {
                (
                    (PROJECT.to_string(), format!("log-{n:08}"), 0),
                    Some(at(*n).timestamp_micros()),
                )
            })
            .collect();
        assert_eq!(keyed::log_rows(&conn, &logs).expect("log rows").len(), 2);
        let scanned = rows_scanned(&s.profile);
        assert!(scanned <= 2 * ROW_GROUP, "log rows scanned {scanned}");

        let ids: Vec<String> = far.iter().map(|n| format!("dp-{n:08}")).collect();
        let instants: Vec<i64> = far.iter().map(|n| at(*n).timestamp_micros()).collect();
        let probe = sideseat_query_sql::dml::metric_winner_probe(
            PROJECT,
            &ids.iter().map(String::as_str).collect::<Vec<_>>(),
            &instants,
        )
        .expect("probe");
        let values: Vec<duckdb::types::Value> = probe
            .params()
            .iter()
            .map(|value| match value {
                sideseat_query_sql::analytics::QueryValue::String(text) => {
                    duckdb::types::Value::Text(text.clone())
                }
                sideseat_query_sql::analytics::QueryValue::Int64(number) => {
                    duckdb::types::Value::BigInt(*number)
                }
                other => panic!("unexpected probe value {other:?}"),
            })
            .collect();
        let found = conn
            .prepare(probe.sql())
            .expect("prepare")
            .query_map(duckdb::params_from_iter(values), |_| Ok(()))
            .expect("probe")
            .count();
        assert_eq!(found, 2);
        let scanned = rows_scanned(&s.profile);
        assert!(scanned <= 2 * ROW_GROUP, "metric probe scanned {scanned}");
    }
}

/// A correction deletes its earlier revision's terms from the row groups they were appended to; a new span
/// deletes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_correction_deletes_only_where_its_revision_wrote() {
    let s = store().await;
    const TERMS: i64 = 500_000;
    const PER_WRITE: i64 = 5_000;
    let start_us = 1_700_000_000_000_000_i64;
    // Terms of a hundred earlier writes, each write's terms appended together under its instant.
    s.service
        .write(|conn| {
            conn.execute_batch(&format!(
                "INSERT INTO span_terms SELECT 'p', lpad(to_hex(i // 35), 32, '0'), lpad(to_hex(i), 16, '0'), \
                 'prompt', 'term' || (i % 997)::VARCHAR, \
                 make_timestamp({start_us} + (i // {PER_WRITE}) * 1000000) FROM range({TERMS}) r(i) ORDER BY i"
            ))
            .map_err(Into::into)
        })
        .expect("terms");
    let revision_us = start_us + 60 * 1_000_000;
    let correction = NormalizedSpan {
        project_id: Some(PROJECT.into()),
        trace_id: format!("{:0>32}", format!("{:x}", 60 * PER_WRITE / 35)),
        span_id: format!("{:0>16}", format!("{:x}", 60 * PER_WRITE)),
        ingested_at: Some(Utc::now()),
        ..Default::default()
    };
    let identity: keyed::SpanIdentity = (
        PROJECT.to_string(),
        correction.trace_id.clone(),
        correction.span_id.clone(),
    );
    s.service
        .write(|conn| {
            super::repositories::search::replace_span_terms(
                conn,
                std::slice::from_ref(&correction),
                &std::collections::HashMap::from([(identity, revision_us)]),
            )
        })
        .expect("replace");
    let scanned = rows_scanned(&s.profile);
    assert!(
        scanned <= 2 * 122_880,
        "the correction's delete scanned {scanned} of {TERMS} term rows"
    );

    std::fs::remove_file(&s.profile).expect("clear the profile");
    s.service
        .write(|conn| {
            super::repositories::search::replace_span_terms(
                conn,
                std::slice::from_ref(&correction),
                &std::collections::HashMap::new(),
            )
        })
        .expect("replace");
    assert!(
        !s.profile.exists(),
        "a span with no stored revision runs no delete"
    );
}

/// Keys that each find many rows - a span corrected five times, whole traces, one span id a client reuses in
/// every project - stay on the index past DuckDB's default limit, so the cost follows the rows asked for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keys_that_find_many_rows_stay_on_the_index() {
    let s = store().await;
    let project = ProjectId::from(PROJECT);
    let start = Utc.timestamp_opt(1_700_000_000, 0).single().expect("start");
    let revised = |revision: usize| -> Vec<NormalizedSpan> {
        (0..600)
            .map(|n| NormalizedSpan {
                project_id: Some(PROJECT.into()),
                trace_id: format!("revised-{n:026}"),
                span_id: format!("revised-{n:08}"),
                content_digest: format!("revision-{revision}"),
                span_name: "step".into(),
                timestamp_start: start,
                ..Default::default()
            })
            .collect()
    };
    for revision in 0..5 {
        s.repo
            .insert_spans(revised(revision))
            .await
            .expect("revisions");
    }
    let shared: Vec<NormalizedSpan> = (0..3_000)
        .map(|n| NormalizedSpan {
            project_id: Some(format!("tenant-{n}")),
            trace_id: format!("shared-trace-{n:019}"),
            span_id: "0000000000000001".into(),
            content_digest: "shared".into(),
            span_name: "step".into(),
            timestamp_start: start,
            ..Default::default()
        })
        .collect();
    s.repo.insert_spans(shared).await.expect("shared span ids");
    let total = (TRACES * SPANS_PER_TRACE + 600 * 5 + 3_000) as u64;

    let check = |name: &str, rows: u64| {
        let scanned = rows_scanned(&s.profile);
        assert!(
            scanned <= rows * 2,
            "{name}: scanned {scanned} of {total} rows for {rows} matching"
        );
    };
    let five_revisions: Vec<(String, String, String)> = (0..512)
        .map(|n| {
            (
                format!("revised-{n:026}"),
                format!("revised-{n:08}"),
                "revision-4".to_string(),
            )
        })
        .collect();
    let matched = s
        .repo
        .spans_with_matching_content(&project, &five_revisions)
        .await
        .expect("revisions");
    assert_eq!(matched.len(), 512);
    check("512 spans with five revisions each", 512 * 5);

    let traces: Vec<String> = (0..512).map(trace).collect();
    s.repo
        .file_reference_fields_for_traces(&project, &traces)
        .await
        .expect("traces");
    check("512 whole traces", (512 * SPANS_PER_TRACE) as u64);

    let found = s
        .repo
        .span_raw_ids(
            &ProjectId::from("tenant-7"),
            &[(
                format!("shared-trace-{:019}", 7),
                "0000000000000001".to_string(),
            )],
        )
        .await
        .expect("shared");
    assert!(found.is_empty(), "no raw record was written for it");
    check("a span id reused in 3,000 projects", 3_000);
}

/// Corrected three times, a span keeps only its last revision's terms: each write replaces the terms of the
/// revision it supersedes, which is why the latest revision's instant is all a correction needs to name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_corrected_span_keeps_only_its_last_revisions_terms() {
    use sideseat_ports::types::{SearchDocument, SearchField, SearchFieldTerms};
    let s = store().await;
    for word in ["first", "second", "third"] {
        s.repo
            .insert_spans(vec![NormalizedSpan {
                project_id: Some(PROJECT.into()),
                trace_id: trace(11),
                span_id: span(11, 3),
                content_digest: word.into(),
                span_name: "step".into(),
                search: SearchDocument {
                    indexed: true,
                    fields: vec![SearchFieldTerms {
                        field: SearchField::Prompt,
                        terms: vec![word.to_string()],
                        truncated: false,
                        text: word.to_string(),
                    }],
                },
                ..Default::default()
            }])
            .await
            .expect("revision");
    }
    let conn = s.service.conn();
    let mut statement = conn
        .prepare("SELECT term FROM span_terms WHERE trace_id = ? AND span_id = ? ORDER BY term")
        .expect("prepare");
    let terms: Vec<String> = statement
        .query_map([trace(11), span(11, 3)], |row| row.get(0))
        .expect("terms")
        .collect::<Result<_, _>>()
        .expect("terms");
    assert_eq!(terms, vec!["third".to_string()]);
}

/// A log record with neither time nor observed time is stored under its delivery's receipt time, which the next
/// delivery's differs from: the redelivery still replaces it, and its confirmation still finds it, because such
/// records are looked up without an instant.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_log_without_a_time_of_its_own_is_replaced_and_confirmed() {
    let s = store().await;
    let project = ProjectId::from(PROJECT);
    let delivered = |received: usize| sideseat_ports::types::NormalizedLog {
        project_id: Some(PROJECT.into()),
        log_digest: "timeless".into(),
        ordinal: 0,
        timestamp: at(received),
        time: None,
        observed_time: None,
        ..Default::default()
    };
    s.repo.insert_logs(&[delivered(10)]).await.expect("first");
    s.repo
        .insert_logs(&[delivered(300_000)])
        .await
        .expect("a redelivery received much later");
    let rows: i64 = s
        .service
        .conn()
        .query_row(
            "SELECT count(*) FROM otel_logs WHERE log_digest = 'timeless'",
            [],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(rows, 1, "the redelivery replaced the stored row");
    assert!(
        s.repo
            .logs_match_content(&project, &[("timeless".to_string(), 0, None)])
            .await
            .expect("confirm")
    );
}

/// A revision older than the stored winner - its instant set earlier, by another clock or a redelivery - does
/// not win, so it neither deletes the winner's terms nor writes its own; a later correction then replaces the
/// winner's, and the span keeps one revision's terms throughout.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_span_keeps_its_winners_terms_whatever_order_revisions_arrive_in() {
    use sideseat_ports::types::{SearchDocument, SearchField, SearchFieldTerms};
    let s = store().await;
    let revision = |word: &str, at_us: i64| NormalizedSpan {
        project_id: Some(PROJECT.into()),
        trace_id: trace(12),
        span_id: span(12, 4),
        content_digest: word.into(),
        span_name: "step".into(),
        ingested_at: chrono::DateTime::from_timestamp_micros(at_us),
        search: SearchDocument {
            indexed: true,
            fields: vec![SearchFieldTerms {
                field: SearchField::Prompt,
                terms: vec![word.to_string()],
                truncated: false,
                text: word.to_string(),
            }],
        },
        ..Default::default()
    };
    let terms = || -> Vec<String> {
        let conn = s.service.conn();
        let mut statement = conn
            .prepare("SELECT term FROM span_terms WHERE trace_id = ? AND span_id = ? ORDER BY term")
            .expect("prepare");
        statement
            .query_map([trace(12), span(12, 4)], |row| row.get(0))
            .expect("terms")
            .collect::<Result<_, _>>()
            .expect("terms")
    };
    s.repo
        .insert_spans(vec![revision("winner", START_US + 2_000_000)])
        .await
        .expect("winner");
    s.repo
        .insert_spans(vec![revision("older", START_US + 1_000_000)])
        .await
        .expect("an older revision, written later");
    assert_eq!(terms(), vec!["winner".to_string()]);
    s.repo
        .insert_spans(vec![revision("correction", START_US + 3_000_000)])
        .await
        .expect("a correction");
    assert_eq!(terms(), vec!["correction".to_string()]);
}

/// A composed request's reads find their rows through the index, not by reading the store.
///
/// The thread key and the call ids are each their scan's only filter, so an index on the column serves it; a
/// project predicate beside the key, or a dropped index, reads every span and fails here. Measured over a store
/// holding thirty thousand spans of which five are the thread's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_composed_request_reads_its_thread_and_its_calls_through_the_index() {
    use sideseat_ports::traits::MessageStore;
    use sideseat_ports::types::RequestContextParams;

    let s = store().await;
    let start = Utc.timestamp_opt(1_700_000_000, 0).single().expect("start");
    let thread = r#"["acme.request_thread","session-1",null,null]"#;
    let requests: Vec<NormalizedSpan> = (0..5)
        .map(|n| NormalizedSpan {
            project_id: Some(PROJECT.into()),
            trace_id: format!("thread-trace-{n:019}"),
            span_id: format!("thread-span-{n:04}"),
            content_digest: format!("thread-{n}"),
            span_name: "acme.request".into(),
            timestamp_start: start + TimeDelta::seconds(n),
            request_thread: thread.to_string(),
            messages: Some("[]".to_string()),
            ..Default::default()
        })
        .collect();
    let calls: Vec<NormalizedSpan> = (0..3)
        .map(|n| NormalizedSpan {
            project_id: Some(PROJECT.into()),
            trace_id: format!("call-trace-{n:021}"),
            span_id: format!("call-span-{n:06}"),
            content_digest: format!("call-{n}"),
            span_name: "acme.tool".into(),
            timestamp_start: start + TimeDelta::seconds(n),
            gen_ai_tool_call_id: Some(format!("call-{n}")),
            messages: Some("[]".to_string()),
            ..Default::default()
        })
        .collect();
    s.repo.insert_spans(requests).await.expect("the thread");
    s.repo.insert_spans(calls).await.expect("the calls");
    let total = (TRACES * SPANS_PER_TRACE + 8) as u64;

    let params = RequestContextParams {
        project_id: ProjectId::from(PROJECT),
        thread: thread.to_string(),
        before_us: (start + TimeDelta::seconds(4)).timestamp_micros(),
        call_ids: vec!["call-0".to_string(), "call-2".to_string()],
        call_trace_ids: (0..3).map(|n| format!("call-trace-{n:021}")).collect(),
        ingested_before_us: None,
    };
    let rows = s
        .repo
        .get_request_context(&params)
        .await
        .expect("the request context");
    assert_eq!(rows.thread.len(), 5, "the thread's own requests");
    assert_eq!(rows.calls.len(), 2, "only the calls asked for");
    let scanned = rows_scanned(&s.profile);
    assert!(
        scanned <= 32,
        "the call read scanned {scanned} of {total} rows for 2 matching, so it read the store"
    );

    // And the thread read alone, profiled on its own statement.
    let thread_only = s
        .repo
        .get_request_context(&RequestContextParams {
            call_ids: Vec::new(),
            ..params
        })
        .await
        .expect("the thread alone");
    assert_eq!(thread_only.thread.len(), 5);
    let scanned = rows_scanned(&s.profile);
    assert!(
        scanned <= 32,
        "the thread read scanned {scanned} of {total} rows for 5 matching, so it read the store"
    );
}
