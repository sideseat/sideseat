/// The trace corpus as exports, each scenario's re-timed into one day ending 80 days back: within ClickHouse's
/// 90-day TTL, and with room after it for a replica a day for 74 days. A scenario keeps its own timing, so a
/// trace whose spans arrive in several exports stays whole.
fn scale_corpus() -> Vec<opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest>
{
    use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
    use prost::Message;

    fn walk(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .expect("fixture directory")
            .map(|entry| entry.expect("entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "pb")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("req-"))
            {
                found.push(path);
            }
        }
    }
    let mut paths = Vec::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/messages"),
        &mut paths,
    );
    let mut scenarios: std::collections::BTreeMap<
        std::path::PathBuf,
        Vec<ExportTraceServiceRequest>,
    > = Default::default();
    for path in paths {
        let bytes = std::fs::read(&path).expect("fixture");
        scenarios
            .entry(path.parent().expect("scenario").to_path_buf())
            .or_default()
            .push(ExportTraceServiceRequest::decode(bytes.as_slice()).expect("an OTLP export"));
    }
    const DAY_NS: u64 = 86_400_000_000_000;
    let base = u64::try_from(
        (Utc::now() - chrono::TimeDelta::days(80))
            .timestamp_nanos_opt()
            .unwrap(),
    )
    .expect("a time after 1970");
    let count = scenarios.len() as u64;
    let mut exports = Vec::new();
    for (index, (_, mut requests)) in scenarios.into_iter().enumerate() {
        let spans = || {
            requests
                .iter()
                .flat_map(|request| &request.resource_spans)
                .flat_map(|resource| &resource.scope_spans)
                .flat_map(|scope| &scope.spans)
        };
        let Some(earliest) = spans()
            .map(|span| span.start_time_unix_nano)
            .filter(|&t| t > 0)
            .min()
        else {
            continue;
        };
        let place = base + index as u64 * (DAY_NS / count);
        let retime = |t: &mut u64| {
            if *t > 0 {
                *t = (*t).saturating_sub(earliest).min(DAY_NS / 2) + place;
            }
        };
        for request in &mut requests {
            for span in request
                .resource_spans
                .iter_mut()
                .flat_map(|resource| &mut resource.scope_spans)
                .flat_map(|scope| &mut scope.spans)
            {
                retime(&mut span.start_time_unix_nano);
                retime(&mut span.end_time_unix_nano);
                for event in &mut span.events {
                    retime(&mut event.time_unix_nano);
                }
            }
        }
        exports.extend(requests);
    }
    exports
}

/// `column` remapped for replica `r.k` in either dialect, the same string in both: the first `width` hex digits
/// of the MD5 of the value and the replica's number. A null or empty value - ClickHouse stores some absent ids as
/// empty strings where DuckDB stores NULL - is kept.
fn scale_identity(backend: &str, column: &str, width: usize) -> String {
    match backend {
        "duckdb" => format!(
            "CASE WHEN {column} IS NULL OR {column} = '' THEN {column} \
             ELSE md5({column} || '-' || r.k::VARCHAR)[1:{width}] END"
        ),
        _ => format!(
            "if(isNull({column}) OR {column} = '', {column}, \
             substring(lower(hex(MD5(concat({column}, '-', toString(r.k))))), 1, {width}))"
        ),
    }
}

/// Every analytics table's replica statement in one dialect, keeping only the columns the store has.
fn scale_statements(
    backend: &str,
    present: &std::collections::HashSet<(String, String)>,
    copies: u64,
) -> Vec<String> {
    let session = |column: &str| match backend {
        "duckdb" => format!(
            "CASE WHEN {column} IS NULL OR {column} = '' THEN {column} ELSE {column} || '-' || r.k::VARCHAR END"
        ),
        _ => format!(
            "if(isNull({column}) OR {column} = '', {column}, concat({column}, '-', toString(r.k)))"
        ),
    };
    let shifted = |column: &str| match backend {
        "duckdb" => format!("{column} + to_days(r.k)"),
        _ => format!("{column} + toIntervalDay(r.k)"),
    };
    let identity = |column: &str, width| scale_identity(backend, column, width);
    let tables: Vec<(&str, Vec<(&str, String)>)> = vec![
        (
            "otel_spans",
            vec![
                ("trace_id", identity("trace_id", 32)),
                ("span_id", identity("span_id", 16)),
                ("parent_span_id", identity("parent_span_id", 16)),
                ("session_id", session("session_id")),
                ("raw_id", identity("raw_id", 64)),
                ("timestamp_start", shifted("timestamp_start")),
                ("timestamp_end", shifted("timestamp_end")),
                ("ingested_at", shifted("ingested_at")),
                ("superseded_at", shifted("superseded_at")),
            ],
        ),
        (
            "span_terms",
            vec![
                ("trace_id", identity("trace_id", 32)),
                ("span_id", identity("span_id", 16)),
                ("ingested_at", shifted("ingested_at")),
            ],
        ),
        (
            "otel_raw",
            vec![
                ("raw_id", identity("raw_id", 64)),
                ("received_at", shifted("received_at")),
            ],
        ),
        (
            "otel_raw_traces",
            vec![
                ("trace_id", identity("trace_id", 32)),
                ("raw_id", identity("raw_id", 64)),
            ],
        ),
    ];
    tables
        .into_iter()
        .filter(|(table, _)| present.iter().any(|(t, _)| t == table))
        .map(|(table, columns)| {
            let rewritten = columns
                .into_iter()
                .filter(|(column, _)| present.contains(&(table.to_string(), (*column).to_string())))
                .map(|(column, expression)| format!("{expression} AS {column}"))
                .collect::<Vec<_>>()
                .join(", ");
            match backend {
                "duckdb" => format!(
                    "INSERT INTO {table} SELECT t.* REPLACE ({rewritten}) FROM {table} t, \
                     (SELECT range AS k FROM range(1, {copies})) r ORDER BY r.k"
                ),
                _ => format!(
                    "INSERT INTO {table} SELECT t.* REPLACE ({rewritten}) FROM {table} AS t \
                     CROSS JOIN (SELECT number AS k FROM numbers(1, {})) AS r",
                    copies - 1
                ),
            }
        })
        .collect()
}

/// The fastest of three runs of a read, with its answer.
async fn fastest_of_three<T, F, Fut>(read: F) -> (std::time::Duration, T)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let mut best: Option<(std::time::Duration, T)> = None;
    for _ in 0..3 {
        let started = std::time::Instant::now();
        let answer = read().await;
        let elapsed = started.elapsed();
        if best.as_ref().is_none_or(|(fastest, _)| elapsed < *fastest) {
            best = Some((elapsed, answer));
        }
    }
    best.expect("three runs")
}

/// Every core read gives the same answer on both backends over the trace corpus grown to a million spans, and
/// the fastest of three runs of each is reported side by side.
///
/// ```text
/// make bench-reads-distributed
/// ```
///
/// The corpus's trace exports go through the trace pipeline into both backends (`scale_corpus`), and each store
/// then copies its own rows into replicas a day apart under fresh identities, hashed alike in both dialects
/// (`scale_statements`), until it holds `SIDESEAT_READ_PARITY_SPANS` spans - a million by default. `ingested_at`
/// is each server's clock at the write, so the reads ordered by it, the feeds, are not compared; every other
/// read is, on the same arguments, sampled from the store.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a measurement at a million spans: make bench-reads-distributed"]
async fn reads_agree_at_a_million_spans() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!(
            "read parity at scale: skipped - set {URL_ENV} (or run `make bench-reads-distributed`)"
        );
        return;
    };
    let target: u64 = std::env::var("SIDESEAT_READ_PARITY_SPANS")
        .ok()
        .map(|spans| spans.parse().expect("a span count"))
        .unwrap_or(1_000_000);
    let database = "sideseat_parity_scale";
    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, database).await;
    let duck_repo: Arc<dyn AnalyticsRepository + Send + Sync> = Arc::new(duck.clone());
    let ch_repo: Arc<dyn AnalyticsRepository + Send + Sync> = Arc::new(ch.clone());

    let exports = scale_corpus();
    for repo in [&duck_repo, &ch_repo] {
        let FilePipeline {
            _temp, pipeline, ..
        } = file_pipeline(repo).await;
        for chunk in exports.chunks(16) {
            let outcomes = pipeline.run_batch_outcomes_for_test(chunk).await;
            assert!(
                outcomes
                    .iter()
                    .all(|outcome| *outcome == sideseat_ingestion::traces::IngestOutcome::Stored),
                "every corpus export is stored: {outcomes:?}"
            );
        }
    }

    // Grow both stores alike.
    let corpus: u64 = duck
        .0
        .conn()
        .query_row("SELECT count(*) FROM otel_spans", [], |row| row.get(0))
        .expect("spans");
    let copies = target.div_ceil(corpus).max(1);
    let started = std::time::Instant::now();
    if copies > 1 {
        let present: std::collections::HashSet<(String, String)> = {
            let conn = duck.0.conn();
            let mut statement = conn
                .prepare("SELECT table_name, column_name FROM information_schema.columns")
                .expect("columns");
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .expect("columns")
                .collect::<Result<_, _>>()
                .expect("columns")
        };
        let sql = format!(
            "SET memory_limit = '4GB'; {}; CHECKPOINT; SET memory_limit = '{}B';",
            scale_statements("duckdb", &present, copies).join(";\n"),
            sideseat_core::constants::DUCKDB_MEMORY_LIMIT_BYTES
        );
        duck.0
            .write(|conn| conn.execute_batch(&sql).map_err(Into::into))
            .expect("grow the DuckDB store");

        let raw = raw_client(&url, database).with_option(
            sideseat_adapter_clickhouse::schema::TENANT_MAINTENANCE_SETTING,
            "1",
        );
        let ch_present: std::collections::HashSet<(String, String)> = raw
            .query("SELECT table, name FROM system.columns WHERE database = currentDatabase()")
            .fetch_all::<(String, String)>()
            .await
            .expect("columns")
            .into_iter()
            .collect();
        for statement in scale_statements("clickhouse", &ch_present, copies) {
            raw.query(&statement)
                .execute()
                .await
                .unwrap_or_else(|error| panic!("grow the ClickHouse store: {error}\n{statement}"));
        }
    }
    println!(
        "grew both stores from {corpus} to {} span rows in {:.0} s",
        corpus * copies,
        started.elapsed().as_secs_f64()
    );

    // Samples, from the store: the median trace, the busiest session, a mid-frequency search term.
    let project = ProjectId::from("default");
    let (trace, session, term, latest_us): (String, String, String, i64) = {
        let conn = duck.0.conn();
        let one = |sql: &str| -> String { conn.query_row(sql, [], |row| row.get(0)).expect(sql) };
        (
            one(
                "SELECT trace_id FROM (SELECT trace_id, count(*) AS n FROM otel_spans \
                 WHERE superseded_at IS NULL GROUP BY 1) ORDER BY n, trace_id \
                 LIMIT 1 OFFSET (SELECT count(DISTINCT trace_id) / 2 FROM otel_spans)",
            ),
            one(
                "SELECT session_id FROM otel_spans WHERE session_id IS NOT NULL AND superseded_at IS NULL \
                 GROUP BY 1 ORDER BY count(*) DESC, 1 LIMIT 1",
            ),
            one(
                "SELECT term FROM span_terms WHERE length(term) > 4 GROUP BY 1 \
                 ORDER BY count(*) DESC, 1 LIMIT 1 OFFSET 50",
            ),
            conn.query_row(
                "SELECT epoch_us(max(timestamp_start)) FROM otel_spans",
                [],
                |row| row.get(0),
            )
            .expect("latest"),
        )
    };
    let latest = DateTime::<Utc>::from_timestamp_micros(latest_us).expect("latest");
    let week = Some(latest - chrono::TimeDelta::days(7));

    let mut timings: Vec<(String, std::time::Duration, std::time::Duration)> = Vec::new();
    let mut mismatches: Vec<String> = Vec::new();
    macro_rules! compare {
        ($name:expr, |$repo:ident| $read:expr, |$answer:ident| $describe:expr) => {{
            let (d_time, d) = fastest_of_three(|| async {
                let $repo = &duck;
                $read.await
            })
            .await;
            let (c_time, c) = fastest_of_three(|| async {
                let $repo = &ch;
                $read.await
            })
            .await;
            let describe = |$answer| $describe;
            let d = describe(d.unwrap_or_else(|error| panic!("{}: duckdb: {error}", $name)));
            let c = describe(c.unwrap_or_else(|error| panic!("{}: clickhouse: {error}", $name)));
            if d != c {
                mismatches.push(format!(
                    "{}:\n  duckdb:     {d:?}\n  clickhouse: {c:?}",
                    $name
                ));
            }
            timings.push(($name.to_string(), d_time, c_time));
        }};
    }

    let traces = |from: Option<DateTime<Utc>>| ListTracesParams {
        project_id: project.clone(),
        page: 1,
        limit: 50,
        include_nongenai: true,
        from_timestamp: from,
        ..Default::default()
    };
    compare!(
        "list traces",
        |repo| repo.list_traces(&traces(None)),
        |page| {
            let (rows, total): (Vec<TraceRow>, u64) = page;
            (total, rows.iter().map(describe_trace).collect::<Vec<_>>())
        }
    );
    compare!(
        "list traces, the latest week",
        |repo| repo.list_traces(&traces(week)),
        |page| {
            let (rows, total): (Vec<TraceRow>, u64) = page;
            (total, rows.iter().map(describe_trace).collect::<Vec<_>>())
        }
    );
    compare!(
        "get trace",
        |repo| repo.get_trace(&project, &trace),
        |row| {
            let row: Option<TraceRow> = row;
            row.as_ref().map(describe_trace)
        }
    );
    compare!(
        "spans of a trace",
        |repo| repo.get_spans_for_trace(&project, &trace, 10_000),
        |rows| {
            let rows: Vec<SpanRow> = rows;
            sorted(rows.iter().map(describe_span).collect())
        }
    );
    let spans_params = ListSpansParams {
        project_id: project.clone(),
        page: 1,
        limit: 50,
        ..Default::default()
    };
    compare!(
        "list spans",
        |repo| repo.list_spans(&spans_params),
        |page| {
            let (rows, total): (Vec<SpanRow>, u64) = page;
            (total, rows.iter().map(describe_span).collect::<Vec<_>>())
        }
    );
    let sessions = |from: Option<DateTime<Utc>>| ListSessionsParams {
        project_id: project.clone(),
        page: 1,
        limit: 50,
        from_timestamp: from,
        ..Default::default()
    };
    compare!(
        "list sessions",
        |repo| repo.list_sessions(&sessions(None)),
        |page| {
            let (rows, total): (Vec<SessionRow>, u64) = page;
            (total, rows.iter().map(describe_session).collect::<Vec<_>>())
        }
    );
    compare!(
        "list sessions, the latest week",
        |repo| repo.list_sessions(&sessions(week)),
        |page| {
            let (rows, total): (Vec<SessionRow>, u64) = page;
            (total, rows.iter().map(describe_session).collect::<Vec<_>>())
        }
    );
    compare!(
        "get session",
        |repo| repo.get_session(&project, &session),
        |row| {
            let row: Option<SessionRow> = row;
            row.as_ref().map(describe_session)
        }
    );
    compare!(
        "traces of a session",
        |repo| repo.get_traces_for_session(&project, &session),
        |rows| {
            let rows: Vec<TraceRow> = rows;
            rows.iter().map(describe_trace).collect::<Vec<_>>()
        }
    );
    for (name, params) in [
        (
            "messages of a trace",
            MessageQueryParams {
                project_id: project.clone(),
                trace_id: Some(trace.clone()),
                ..Default::default()
            },
        ),
        (
            "messages of a session",
            MessageQueryParams {
                project_id: project.clone(),
                session_id: Some(session.clone()),
                ..Default::default()
            },
        ),
    ] {
        compare!(name, |repo| repo.get_messages(&params), |result| {
            let result: sideseat_ports::types::MessageQueryResult = result;
            sorted(result.rows.iter().map(describe_message_row).collect())
        });
    }
    let options =
        |map: std::collections::HashMap<String, Vec<sideseat_ports::traits::FilterOptionRow>>| {
            let mut described: Vec<String> = map
                .into_iter()
                .map(|(column, rows)| {
                    let mut values: Vec<String> = rows
                        .iter()
                        .map(|row| format!("{}={}", row.value, row.count))
                        .collect();
                    values.sort();
                    format!("{column}: {}", values.join(","))
                })
                .collect();
            described.sort();
            described
        };
    let trace_columns: Vec<String> = sideseat_ports::types::TRACE_FILTER_OPTION_COLUMNS
        .iter()
        .map(|(view_column, _)| view_column.to_string())
        .collect();
    compare!(
        "trace filter options",
        |repo| repo.get_trace_filter_options(&project, &trace_columns, week, None),
        |map| options(map)
    );
    let span_columns: Vec<String> = sideseat_ports::types::SPAN_FILTER_OPTION_COLUMNS
        .iter()
        .map(|column| column.to_string())
        .collect();
    compare!(
        "span filter options",
        |repo| repo.get_span_filter_options(&project, &span_columns, week, None, false),
        |map| options(map)
    );
    compare!(
        "session filter options",
        |repo| repo.get_session_filter_options(&project, &["environment".to_string()], week, None),
        |map| options(map)
    );
    let stats = sideseat_ports::types::StatsParams {
        project_id: project.clone(),
        from_timestamp: latest - chrono::TimeDelta::days(7),
        to_timestamp: latest,
        timezone: "UTC".parse().unwrap(),
    };
    compare!(
        "project stats",
        |repo| repo.get_project_stats(&stats),
        |result| { format!("{result:?}") }
    );
    let search = SearchQuery {
        project_id: project.clone(),
        signal: SearchSignal::Spans,
        expression: sideseat_ports::types::SearchExpr::Term {
            field: None,
            term: term.clone(),
        },
        limit: 50,
        max_examined: 1_000,
        cursor: None,
        from_timestamp: None,
        to_timestamp: None,
    };
    compare!("search spans", |repo| repo.search(&search), |page| {
        let page: sideseat_ports::types::SearchPage = page;
        page.candidates
            .iter()
            .map(|candidate| match &candidate.record {
                SearchRecord::Span(span) => format!("{}/{}", span.trace_id, span.span_id),
                SearchRecord::Log(log) => format!("{log:?}"),
            })
            .collect::<Vec<_>>()
    });

    println!(
        "\n{} span rows; trace {trace}; session {session}; term {term}\n\
         | read | DuckDB | ClickHouse |\n|---|---|---|",
        corpus * copies
    );
    for (name, duck_time, ch_time) in &timings {
        println!(
            "| {name} | {:.1} ms | {:.1} ms |",
            duck_time.as_secs_f64() * 1000.0,
            ch_time.as_secs_f64() * 1000.0
        );
    }
    assert!(
        mismatches.is_empty(),
        "{} read(s) answer differently:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}
