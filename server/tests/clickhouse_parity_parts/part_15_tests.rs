/// The trace corpus as exports, each scenario's re-timed into one day ending 80 days back: within ClickHouse's
/// 90-day TTL, and with room after it for a replica a day for 74 days. A scenario keeps its own timing, so a
/// trace whose spans arrive in several exports stays whole, and its logs keep their place beside its spans.
fn scale_corpus() -> (
    Vec<opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest>,
    Vec<opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest>,
) {
    use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
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
                    .is_some_and(|name| name.starts_with("req-") || name.starts_with("logs-"))
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
    type Scenario = (
        Vec<ExportTraceServiceRequest>,
        Vec<ExportLogsServiceRequest>,
    );
    let mut scenarios: std::collections::BTreeMap<std::path::PathBuf, Scenario> =
        Default::default();
    for path in paths {
        let bytes = std::fs::read(&path).expect("fixture");
        let scenario = scenarios
            .entry(path.parent().expect("scenario").to_path_buf())
            .or_default();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("logs-"))
        {
            scenario
                .1
                .push(ExportLogsServiceRequest::decode(bytes.as_slice()).expect("an OTLP export"));
        } else {
            scenario
                .0
                .push(ExportTraceServiceRequest::decode(bytes.as_slice()).expect("an OTLP export"));
        }
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
    let mut log_exports = Vec::new();
    for (index, (_, (mut requests, mut logs))) in scenarios.into_iter().enumerate() {
        let span_times = requests
            .iter()
            .flat_map(|request| &request.resource_spans)
            .flat_map(|resource| &resource.scope_spans)
            .flat_map(|scope| &scope.spans)
            .map(|span| span.start_time_unix_nano);
        let log_times = logs
            .iter()
            .flat_map(|request| &request.resource_logs)
            .flat_map(|resource| &resource.scope_logs)
            .flat_map(|scope| &scope.log_records)
            .flat_map(|record| [record.time_unix_nano, record.observed_time_unix_nano]);
        let Some(earliest) = span_times.chain(log_times).filter(|&t| t > 0).min() else {
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
        for request in &mut logs {
            for record in request
                .resource_logs
                .iter_mut()
                .flat_map(|resource| &mut resource.scope_logs)
                .flat_map(|scope| &mut scope.log_records)
            {
                retime(&mut record.time_unix_nano);
                retime(&mut record.observed_time_unix_nano);
            }
        }
        exports.extend(requests);
        log_exports.extend(logs);
    }
    (exports, log_exports)
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
        (
            "otel_logs",
            vec![
                ("log_digest", identity("log_digest", 64)),
                ("trace_id", identity("trace_id", 32)),
                ("span_id", identity("span_id", 16)),
                ("session_id", session("session_id")),
                ("timestamp", shifted("timestamp")),
                ("time", shifted("time")),
                ("observed_time", shifted("observed_time")),
                ("ingested_at", shifted("ingested_at")),
            ],
        ),
        (
            "log_media_refs",
            vec![
                ("log_digest", identity("log_digest", 64)),
                ("trace_id", identity("trace_id", 32)),
                ("span_id", identity("span_id", 16)),
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

/// Every field of a log row but its receipt time, which is each server's clock at the write; JSON-valued ones
/// canonical, since the dialects may order an object's members differently.
fn describe_log(row: &sideseat_ports::types::LogRow) -> String {
    let json = |value: &Option<String>| value.as_deref().map(canonical_json);
    format!(
        "{}/{} at={} time={:?} observed={:?} severity={}/{:?} body={:?} text={:?} attributes={:?} \
         dropped={} flags={} trace={:?} span={:?} event={:?} session={:?} user={:?} env={:?} \
         service={:?}/{:?}/{:?}/{:?} resource={:?} scope={:?}/{:?} scope_attributes={:?} \
         schemas={:?}/{:?} raw={:?}",
        row.log_digest,
        row.ordinal,
        row.timestamp.timestamp_micros(),
        row.time.map(|t| t.timestamp_micros()),
        row.observed_time.map(|t| t.timestamp_micros()),
        row.severity_number,
        row.severity_text,
        json(&row.body),
        row.body_text,
        json(&row.attributes),
        row.dropped_attributes_count,
        row.flags,
        row.trace_id,
        row.span_id,
        row.event_name,
        row.session_id,
        row.user_id,
        row.environment,
        row.service_name,
        row.service_version,
        row.service_namespace,
        row.service_instance_id,
        json(&row.resource_attributes),
        row.scope_name,
        row.scope_version,
        json(&row.scope_attributes),
        row.scope_schema_url,
        row.resource_schema_url,
        json(&row.raw_log),
    )
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

    let (exports, log_exports) = scale_corpus();
    let project_name = "default";
    for repo in [&duck_repo, &ch_repo] {
        let FilePipeline {
            _temp,
            pipeline,
            transactional,
            files,
            ..
        } = file_pipeline(repo).await;
        for request in &log_exports {
            // As the collector receives it: the project named on the resource.
            let mut request = request.clone();
            sideseat_ingestion::otlp::inject_project_id_logs(&mut request, project_name);
            sideseat_ingestion::logs::ingest(
                &request,
                repo.as_ref(),
                transactional.as_ref(),
                Some(&files),
                Utc::now(),
            )
            .await
            .expect("every corpus logs export is stored");
        }
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
    // The median log by its identity's hash, and a mid-frequency term over every field logs search.
    let (log_digest, log_ordinal, log_term): (String, u32, String) = {
        let conn = duck.0.conn();
        let (digest, ordinal) = conn
            .query_row(
                "SELECT log_digest, ordinal FROM otel_logs ORDER BY hash(log_digest), log_digest, ordinal \
                 LIMIT 1 OFFSET (SELECT count(*) / 2 FROM otel_logs)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("a log");
        let term = conn
            .query_row(
                "SELECT term FROM (SELECT unnest(string_split(trim(terms), ' ')) AS term FROM (\
                 SELECT search_body AS terms FROM otel_logs UNION ALL SELECT search_event_name FROM otel_logs \
                 UNION ALL SELECT search_severity FROM otel_logs UNION ALL SELECT search_attributes FROM otel_logs) \
                 WHERE terms IS NOT NULL) WHERE length(term) > 4 GROUP BY 1 \
                 ORDER BY count(*) DESC, 1 LIMIT 1 OFFSET 50",
                [],
                |row| row.get(0),
            )
            .expect("a log term");
        (digest, ordinal, term)
    };
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

    let log_search = SearchQuery {
        signal: SearchSignal::Logs,
        expression: sideseat_ports::types::SearchExpr::Term {
            field: None,
            term: log_term.clone(),
        },
        ..search.clone()
    };
    // The store's own candidate read, and below the answer the API gives over it.
    compare!(
        "search logs, the store's candidates",
        |repo| repo.search(&log_search),
        |page| {
            let page: sideseat_ports::types::SearchPage = page;
            page.candidates
                .iter()
                .map(|candidate| format!("{:?}", candidate.record))
                .collect::<Vec<_>>()
        }
    );
    // The log reads as the API answers them: through the domain, which rebuilds the body views a store keeps only
    // in the raw record (`log_views`).
    let duck_view = sideseat_domain::dedup::DedupAnalyticsRepository::new(Box::new(duck.clone()));
    let ch_view = sideseat_domain::dedup::DedupAnalyticsRepository::new(Box::new(ch.clone()));
    macro_rules! compare_views {
        ($name:expr, |$repo:ident| $read:expr, |$answer:ident| $describe:expr) => {{
            let (d_time, d) = fastest_of_three(|| async {
                let $repo = &duck_view;
                $read.await
            })
            .await;
            let (c_time, c) = fastest_of_three(|| async {
                let $repo = &ch_view;
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
    let logs = sideseat_ports::types::ListLogsParams {
        project_id: project.clone(),
        page: 1,
        limit: 50,
        ..Default::default()
    };
    compare_views!("list logs", |repo| repo.list_logs(&logs), |page| {
        let (rows, total): (Vec<sideseat_ports::types::LogRow>, u64) = page;
        (total, rows.iter().map(describe_log).collect::<Vec<_>>())
    });
    compare_views!(
        "get log",
        |repo| repo.get_log(&project, &log_digest, log_ordinal),
        |row| {
            let row: Option<sideseat_ports::types::LogRow> = row;
            row.as_ref().map(describe_log)
        }
    );
    compare_views!(
        "search logs",
        |repo| sideseat_domain::search::SearchService::execute(repo, &log_search),
        |page| {
            let page: sideseat_domain::search::SearchResultPage = page;
            page.hits
                .iter()
                .map(|hit| format!("{:?} {:?}", hit.record, hit.fragments))
                .collect::<Vec<_>>()
        }
    );

    println!(
        "\n{} span rows; trace {trace}; session {session}; term {term}; log term {log_term}\n\
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
