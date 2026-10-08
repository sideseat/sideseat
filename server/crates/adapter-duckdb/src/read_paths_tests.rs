//! Latency and rows scanned for every read the API and the ingest path make, over a store on disk: the before and
//! after of a storage change, measured on the same data.
//!
//! ```text
//! SIDESEAT_READ_PATHS_STORE=<a data directory holding duckdb/sideseat.duckdb> \
//!     cargo test --locked -p sideseat-adapter-duckdb --release --lib read_paths -- --ignored --nocapture
//! ```
//!
//! The store is copied first, so the writes measured at the end change nothing. Samples - the busiest project,
//! a trace of median size, a session, identities to confirm - are drawn from the store itself. Rows scanned are
//! DuckDB's own profile of the *last* statement each read runs, which is the whole read for the single-statement
//! ones; latency is the fastest of nine runs after a warm-up, since a loaded machine only ever adds time.
//!
//! Two more variables make it the `make bench-reads` gate. `SIDESEAT_READ_PATHS_SPANS` grows the copy to at
//! least that many span rows first (`read_paths_store`). `SIDESEAT_READ_PATHS_CEILINGS` names a JSON object
//! of each read's latency ceiling in milliseconds: the run then fails when any read fails - running out of the
//! production memory limit among them, since the store is opened as the server opens it - or is slower than
//! its ceiling, and when a read has no ceiling or a ceiling no read.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{TimeDelta, Utc};
use sideseat_core::constants::DUCKDB_DB_FILENAME;
use sideseat_core::storage::{AppStorage, DataSubdir};
use sideseat_ports::traits::{
    EntityQuery, LogStore, MessageStore, MetricStore, RawStore, SearchIndex, SpanStore,
    SurvivorReferences,
};
use sideseat_ports::types::{
    FeedMessagesParams, FeedSpansParams, ListLogsParams, ListMetricsParams, ListSessionsParams,
    ListSpansParams, ListTracesParams, MessageQueryParams, ProjectId, SearchExpr, SearchQuery,
    SearchSignal, StatsParams,
};

use super::{DuckdbRepository, DuckdbService, TestClock};

struct Samples {
    project: ProjectId,
    trace: String,
    /// The busiest project that has sessions, and its busiest session: the busiest project overall may have none.
    session_project: ProjectId,
    session: String,
    spans: Vec<(String, String, String)>,
    raw_ids: Vec<String>,
    log: Option<(String, u32, chrono::DateTime<Utc>)>,
    datapoint: Option<(String, String, chrono::DateTime<Utc>)>,
    metric_name: Option<String>,
    term: Option<String>,
}

fn one<T: duckdb::types::FromSql>(conn: &duckdb::Connection, sql: &str) -> Option<T> {
    conn.query_row(sql, [], |row| row.get::<_, Option<T>>(0))
        .ok()
        .flatten()
}

fn strings(conn: &duckdb::Connection, sql: &str) -> Vec<String> {
    let mut statement = conn.prepare(sql).expect("prepare");
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows")
}

fn samples(service: &DuckdbService) -> Samples {
    let conn = service.conn();
    let project: String = one(
        &conn,
        "SELECT project_id FROM otel_spans GROUP BY 1 ORDER BY count(*) DESC, 1 LIMIT 1",
    )
    .expect("a store with spans");
    let p = project.replace('\'', "''");
    let trace: String = one(
        &conn,
        &format!(
            "SELECT trace_id FROM (SELECT trace_id, count(*) AS n FROM otel_spans WHERE project_id = '{p}' \
             GROUP BY 1) ORDER BY n, trace_id LIMIT 1 OFFSET (SELECT count(DISTINCT trace_id) / 2 FROM \
             otel_spans WHERE project_id = '{p}')"
        ),
    )
    .expect("a trace");
    let session_project: String = one(
        &conn,
        "SELECT project_id FROM otel_spans GROUP BY 1 HAVING count(session_id) > 0 \
         ORDER BY count(*) DESC, 1 LIMIT 1",
    )
    .expect("a store with a session");
    let session: String = one(
        &conn,
        &format!(
            "SELECT session_id FROM otel_spans WHERE project_id = '{}' AND session_id IS NOT NULL \
             GROUP BY 1 ORDER BY count(*) DESC, 1 LIMIT 1",
            session_project.replace('\'', "''")
        ),
    )
    .expect("a session");
    let mut statement = conn
        .prepare(&format!(
            "SELECT trace_id, span_id, content_digest FROM otel_spans WHERE project_id = '{p}' \
             ORDER BY hash(trace_id || span_id), trace_id, span_id LIMIT 5"
        ))
        .expect("prepare");
    let spans = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("spans")
        .collect::<Result<Vec<_>, _>>()
        .expect("spans");
    let raw_ids = strings(
        &conn,
        &format!(
            "SELECT raw_id FROM (SELECT DISTINCT raw_id FROM otel_spans WHERE project_id = '{p}' \
             AND raw_id IS NOT NULL) ORDER BY hash(raw_id), raw_id LIMIT 5"
        ),
    );
    let instant = |us: i64| chrono::DateTime::from_timestamp_micros(us).expect("instant");
    let log = conn
        .query_row(
            "SELECT log_digest, ordinal, epoch_us(\"timestamp\") FROM otel_logs \
             ORDER BY hash(log_digest), log_digest, ordinal LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u32>(1)?,
                    instant(row.get::<_, i64>(2)?),
                ))
            },
        )
        .ok();
    let datapoint = conn
        .query_row(
            "SELECT datapoint_id, content_digest, epoch_us(\"timestamp\") FROM otel_metrics \
             ORDER BY hash(datapoint_id), datapoint_id LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    instant(row.get::<_, i64>(2)?),
                ))
            },
        )
        .ok();
    let metric_name = one(
        &conn,
        "SELECT metric_name FROM otel_metrics GROUP BY 1 ORDER BY count(*) DESC, 1 LIMIT 1",
    );
    let term = one(
        &conn,
        &format!(
            "SELECT term FROM span_terms WHERE project_id = '{p}' AND length(term) > 4 GROUP BY 1 \
             ORDER BY count(*) DESC, 1 LIMIT 1 OFFSET 50"
        ),
    );
    Samples {
        project: ProjectId::from(project.as_str()),
        trace,
        session_project: ProjectId::from(session_project.as_str()),
        session,
        spans,
        raw_ids,
        log,
        datapoint,
        metric_name,
        term,
    }
}

/// Rows scanned by the last statement profiled, summed over its scans.
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
    let Ok(text) = std::fs::read_to_string(profile) else {
        return 0;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return 0;
    };
    let mut total = 0;
    walk(&json, &mut total);
    total
}

macro_rules! measure {
    ($rows:ident, $profile:expr, $name:expr, $body:expr) => {{
        let mut outcome = Ok(());
        let mut runs: Vec<Duration> = Vec::new();
        for _ in 0..10 {
            let started = Instant::now();
            let result = $body.await;
            runs.push(started.elapsed());
            if let Err(error) = result {
                outcome = Err(error.to_string());
                break;
            }
        }
        match outcome {
            Ok(()) => {
                // The fastest of nine after a warm-up: the shared machine's load only ever adds time, so the
                // minimum is the figure two runs can be compared by.
                let timed = runs.split_off(1);
                let fastest = timed.into_iter().min().expect("timed runs");
                $rows.push(($name.to_string(), Ok(fastest), rows_scanned($profile)));
            }
            Err(error) => $rows.push(($name.to_string(), Err(error), 0)),
        }
    }};
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "a measurement over a store on disk; see the module documentation"]
async fn read_paths() {
    let Ok(source) = std::env::var("SIDESEAT_READ_PATHS_STORE") else {
        eprintln!("read paths: set SIDESEAT_READ_PATHS_STORE to a data directory");
        return;
    };
    let temp = tempfile::TempDir::new().expect("temp dir");
    let storage = AppStorage::init_for_test(temp.path().to_path_buf());
    let directory = storage.subdir(DataSubdir::Duckdb);
    std::fs::create_dir_all(&directory).expect("duckdb dir");
    std::fs::copy(
        std::path::Path::new(&source)
            .join("duckdb")
            .join(DUCKDB_DB_FILENAME),
        directory.join(DUCKDB_DB_FILENAME),
    )
    .expect("copy the store");
    let service = Arc::new(
        DuckdbService::init(&storage, Arc::new(TestClock))
            .await
            .expect("open the store"),
    );
    if let Ok(spans) = std::env::var("SIDESEAT_READ_PATHS_SPANS") {
        let spans: u64 = spans
            .parse()
            .expect("SIDESEAT_READ_PATHS_SPANS is a span count");
        let started = Instant::now();
        let grown = super::read_paths_store::replicate_to(&service, spans);
        println!(
            "grew the store to {grown} span rows in {:.0} s",
            started.elapsed().as_secs_f64()
        );
    }
    let s = samples(&service);
    let profile = temp.path().join("profile.json");
    {
        let conn = service.conn();
        conn.execute_batch(&format!(
            "PRAGMA enable_profiling = 'json'; PRAGMA profiling_output = '{}';",
            profile.display()
        ))
        .expect("profiling");
    }
    let repo = DuckdbRepository(Arc::clone(&service));
    let project = &s.project;
    let now = Utc::now();
    let month = Some(now - TimeDelta::days(3650));
    let mut rows: Vec<(String, Result<Duration, String>, u64)> = Vec::new();
    let traces = vec![s.trace.clone()];
    let pairs: Vec<(String, String)> = s
        .spans
        .iter()
        .map(|(trace, span, _)| (trace.clone(), span.clone()))
        .collect();

    measure!(
        rows,
        &profile,
        "api: list traces",
        repo.list_traces(&ListTracesParams {
            project_id: project.clone(),
            page: 1,
            limit: 50,
            ..Default::default()
        })
    );
    measure!(
        rows,
        &profile,
        "api: get trace",
        repo.get_trace(project, &s.trace)
    );
    measure!(
        rows,
        &profile,
        "api: spans of a trace",
        repo.get_spans_for_trace(project, &s.trace, 10_000)
    );
    measure!(
        rows,
        &profile,
        "api: get span",
        repo.get_span(project, &s.spans[0].0, &s.spans[0].1)
    );
    measure!(
        rows,
        &profile,
        "api: list spans",
        repo.list_spans(&ListSpansParams {
            project_id: project.clone(),
            page: 1,
            limit: 50,
            ..Default::default()
        })
    );
    measure!(
        rows,
        &profile,
        "api: list spans of a trace",
        repo.list_spans(&ListSpansParams {
            project_id: project.clone(),
            page: 1,
            limit: 50,
            trace_id: Some(s.trace.clone()),
            ..Default::default()
        })
    );
    measure!(
        rows,
        &profile,
        "api: feed spans",
        repo.get_feed_spans(&FeedSpansParams {
            project_id: project.clone(),
            limit: 50,
            ..Default::default()
        })
    );
    measure!(
        rows,
        &profile,
        "api: span counts",
        repo.get_span_counts_bulk(project, &pairs)
    );
    measure!(
        rows,
        &profile,
        "api: list sessions",
        repo.list_sessions(&ListSessionsParams {
            project_id: s.session_project.clone(),
            page: 1,
            limit: 50,
            ..Default::default()
        })
    );
    let (session_project, session) = (&s.session_project, &s.session);
    measure!(
        rows,
        &profile,
        "api: get session",
        repo.get_session(session_project, session)
    );
    measure!(
        rows,
        &profile,
        "api: traces of a session",
        repo.get_traces_for_session(session_project, session)
    );
    measure!(
        rows,
        &profile,
        "api: messages of a session",
        repo.get_messages(&MessageQueryParams {
            project_id: session_project.clone(),
            session_id: Some(session.clone()),
            ..Default::default()
        })
    );
    measure!(
        rows,
        &profile,
        "api: trace ids of sessions",
        repo.get_trace_ids_for_sessions(session_project, std::slice::from_ref(session), None)
    );
    measure!(
        rows,
        &profile,
        "api: session ids of traces",
        repo.get_session_ids_for_traces(project, &traces, None)
    );
    measure!(
        rows,
        &profile,
        "api: trace filter options",
        repo.get_trace_filter_options(
            project,
            &["environment".to_string(), "framework".to_string()],
            month,
            None
        )
    );
    measure!(
        rows,
        &profile,
        "api: span filter options",
        repo.get_span_filter_options(
            project,
            &[
                "span_category".to_string(),
                "gen_ai_request_model".to_string()
            ],
            month,
            None,
            false
        )
    );
    measure!(
        rows,
        &profile,
        "api: session filter options",
        repo.get_session_filter_options(session_project, &["environment".to_string()], month, None)
    );
    measure!(
        rows,
        &profile,
        "api: trace tags",
        repo.get_trace_tags_options(project, month, None)
    );
    // A week of the store's own data, so the figure is the data's and not an empty range's.
    let latest_us: i64 = one(
        &service.conn(),
        &format!(
            "SELECT epoch_us(max(timestamp_start)) FROM otel_spans WHERE project_id = '{project}'"
        ),
    )
    .unwrap_or_default();
    let latest = chrono::DateTime::from_timestamp_micros(latest_us).unwrap_or(now);
    measure!(
        rows,
        &profile,
        "api: project stats",
        repo.get_project_stats(&StatsParams {
            project_id: project.clone(),
            from_timestamp: latest - TimeDelta::days(7),
            to_timestamp: latest,
            timezone: chrono_tz::UTC,
        })
    );
    measure!(
        rows,
        &profile,
        "api: messages of a trace",
        repo.get_messages(&MessageQueryParams {
            project_id: project.clone(),
            trace_id: Some(s.trace.clone()),
            ..Default::default()
        })
    );
    measure!(
        rows,
        &profile,
        "api: project message feed",
        repo.get_project_messages(&FeedMessagesParams {
            project_id: project.clone(),
            limit: 20,
            ..Default::default()
        })
    );
    if let Some(term) = &s.term {
        measure!(
            rows,
            &profile,
            "api: search spans",
            repo.search(&SearchQuery {
                project_id: project.clone(),
                signal: SearchSignal::Spans,
                expression: SearchExpr::Term {
                    field: None,
                    term: term.clone(),
                },
                limit: 50,
                max_examined: 10_000,
                cursor: None,
                from_timestamp: None,
                to_timestamp: None,
            })
        );
    }
    measure!(
        rows,
        &profile,
        "api: list logs",
        repo.list_logs(&ListLogsParams {
            project_id: project.clone(),
            page: 1,
            limit: 50,
            ..Default::default()
        })
    );
    if let Some((digest, ordinal, log_instant)) = &s.log {
        measure!(
            rows,
            &profile,
            "api: get log",
            repo.get_log(project, digest, *ordinal)
        );
        measure!(
            rows,
            &profile,
            "ingest: logs match content",
            repo.logs_match_content(project, &[(digest.clone(), *ordinal, Some(*log_instant))])
        );
    }
    measure!(
        rows,
        &profile,
        "api: list metrics",
        repo.list_metrics(&ListMetricsParams {
            project_id: project.clone(),
            page: 1,
            limit: 50,
            metric_name: s.metric_name.clone(),
            ..Default::default()
        })
    );
    measure!(
        rows,
        &profile,
        "api: aggregate metrics",
        repo.aggregate_metrics(&ListMetricsParams {
            project_id: project.clone(),
            page: 1,
            limit: 50,
            ..Default::default()
        })
    );
    if let Some((datapoint, content_digest, metric_instant)) = &s.datapoint {
        measure!(
            rows,
            &profile,
            "api: get metric",
            repo.get_metric(project, datapoint)
        );
        measure!(
            rows,
            &profile,
            "ingest: metrics match content",
            repo.metrics_match_content(
                project,
                &[(datapoint.clone(), content_digest.clone(), *metric_instant)]
            )
        );
    }

    measure!(
        rows,
        &profile,
        "ingest: spans match content",
        repo.spans_match_content(project, &s.spans)
    );
    measure!(
        rows,
        &profile,
        "ingest: spans with matching content",
        repo.spans_with_matching_content(project, &s.spans)
    );
    measure!(
        rows,
        &profile,
        "ingest: span raw ids",
        repo.span_raw_ids(project, &pairs)
    );
    measure!(
        rows,
        &profile,
        "ingest: raw records",
        repo.get_raw_records(project, &s.raw_ids)
    );
    measure!(
        rows,
        &profile,
        "ingest: raw records named",
        repo.raw_records_named(project, &s.raw_ids)
    );
    measure!(
        rows,
        &profile,
        "ingest: traces without spans",
        repo.traces_without_spans(project, &traces)
    );
    measure!(
        rows,
        &profile,
        "ingest: file reference fields",
        repo.file_reference_fields_for_traces(project, &traces)
    );
    measure!(
        rows,
        &profile,
        "ingest: survivor raw records",
        repo.survivor_raw_records(project, &traces)
    );

    let spans_total: u64 = one(&service.conn(), "SELECT count(*) FROM otel_spans").unwrap_or(0);
    println!(
        "\nstore: {spans_total} span rows; project {project}; trace {}",
        s.trace
    );
    let ceilings: Option<std::collections::BTreeMap<String, f64>> =
        std::env::var("SIDESEAT_READ_PATHS_CEILINGS")
            .ok()
            .map(|path| {
                serde_json::from_str(&std::fs::read_to_string(&path).expect("the ceilings file"))
                    .expect("the ceilings: read name to milliseconds")
            });
    let mut failures = Vec::new();
    println!("| read | fastest | ceiling | rows scanned (last statement) |");
    println!("|---|---|---|---|");
    for (name, median, scanned) in &rows {
        let ceiling = ceilings.as_ref().and_then(|ceilings| ceilings.get(name));
        let ceiling_text = ceiling.map_or(String::new(), |ms| format!("{ms:.0} ms"));
        match median {
            Ok(median) => {
                let ms = median.as_secs_f64() * 1000.0;
                println!("| {name} | {ms:.2} ms | {ceiling_text} | {scanned} |");
                match (ceilings.as_ref(), ceiling) {
                    (Some(_), None) => failures.push(format!("{name}: no ceiling")),
                    (_, Some(&ceiling)) if ms > ceiling => {
                        failures.push(format!(
                            "{name}: {ms:.2} ms over its {ceiling:.0} ms ceiling"
                        ));
                    }
                    _ => {}
                }
            }
            Err(error) => {
                println!(
                    "| {name} | fails: {} | {ceiling_text} | |",
                    error.chars().take(90).collect::<String>()
                );
                failures.push(format!("{name}: {error}"));
            }
        }
    }
    if let Some(ceilings) = &ceilings {
        for name in ceilings.keys() {
            if !rows.iter().any(|(read, _, _)| read == name) {
                failures.push(format!("{name}: a ceiling for a read that did not run"));
            }
        }
        assert!(
            failures.is_empty(),
            "{} read(s) failed the gate:\n{}",
            failures.len(),
            failures.join("\n")
        );
        println!("every read completed within the memory limit and its ceiling");
    }
}
