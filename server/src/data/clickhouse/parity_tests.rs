//! ClickHouse/DuckDB read-path parity.
//!
//! The two analytics backends implement the same `AnalyticsRepository` over hand-written SQL in
//! two dialects. Nothing but review kept them agreeing, and review does not catch a ClickHouse
//! expression that is merely *accepted* while returning something different: an arbitrary span's
//! tags instead of the union, a null trace name where DuckDB falls back to the earliest named
//! span, a `max()` over a Nullable that stays Nullable. Some do not even parse, and that only
//! shows up against a real server - `JSONExtract` on a `Nullable(String)` inside an array fails
//! with "Nested type Array(String) cannot be inside Nullable type" and takes the whole trace
//! list with it.
//!
//! So: insert one span set into both backends, call the analytics read methods on both, and
//! require the answers to match. Covered: trace list and single trace, span list, spans for a
//! trace, single span, events, links, bulk span counts, session list and single session, traces
//! and trace ids for a session, message rows for span/trace/session, the project feed's span and
//! message pages, filter options for all three scopes, tag options, project span counts and
//! project stats. Not covered: the delete paths and metric ingestion. DuckDB is the reference because it is the default backend and its behaviour
//! is what the goldens and the UI were built against.
//!
//! Needs a live ClickHouse. Skips with a message when `SIDESEAT_TEST_CLICKHOUSE_URL` is unset,
//! so `cargo test` stays green on a checkout with no container:
//!
//! ```bash
//! make test-clickhouse     # starts a container, runs this, removes it
//! ```

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};

use crate::core::config::ClickhouseConfig;
use crate::core::storage::AppStorage;
use crate::data::clickhouse::ClickhouseService;
use crate::data::duckdb::DuckdbService;
use crate::data::traits::AnalyticsRepository;
use crate::data::types::{
    ListSessionsParams, ListSpansParams, ListTracesParams, MessageQueryParams, MessageSpanRow,
    NormalizedSpan, ObservationType, SessionRow, SpanCategory, SpanRow, TraceRow,
};

/// Env var holding the base URL of a ClickHouse HTTP endpoint, e.g. `http://127.0.0.1:8123`.
const URL_ENV: &str = "SIDESEAT_TEST_CLICKHOUSE_URL";
/// Credentials, when the server requires them. Recent official images generate a random password
/// for `default` and reject unauthenticated queries outright.
const USER_ENV: &str = "SIDESEAT_TEST_CLICKHOUSE_USER";
const PASSWORD_ENV: &str = "SIDESEAT_TEST_CLICKHOUSE_PASSWORD";

const PROJECT: &str = "parity";

/// Fixture timestamps, relative to a base fixed once per run.
///
/// Deliberately recent: the ClickHouse schema carries `TTL timestamp_start + toIntervalDay(90)`,
/// so a part whose rows are all older than the retention window is dropped at insert time. A
/// fixture with hardcoded 2025 timestamps therefore vanished from ClickHouse and stayed in
/// DuckDB, which has no TTL - the first thing this test caught. Truncated to whole seconds so
/// neither dialect's sub-second handling can read as a mismatch.
fn ts(secs: i64) -> DateTime<Utc> {
    static BASE: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    let base = *BASE.get_or_init(|| Utc::now().timestamp() - 3600);
    Utc.timestamp_opt(base + secs, 0).unwrap()
}

/// A span set chosen for the cases where the two dialects can disagree, not for realism:
///
/// - `trace-a`: root + two child generation spans. Exercises token dedup (the parent must not
///   double-count its children) and the tags union across spans with different tags.
/// - `trace-b`: same session as `trace-a`, so session aggregation spans two traces.
/// - `trace-c`: **no root span**, so `trace_name` must come from the earliest named span. This is
///   the fallback whose absence made ClickHouse return a null name where DuckDB returned one.
/// - `trace-d`: no session, no generation span, an error status, and no tags - the all-defaults
///   path where tokens and costs must read 0 rather than NULL.
fn fixture_spans() -> Vec<NormalizedSpan> {
    let base = |trace: &str, span: &str, name: &str, offset: i64| NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: trace.to_string(),
        span_id: span.to_string(),
        span_name: name.to_string(),
        timestamp_start: ts(offset),
        timestamp_end: Some(ts(offset + 1)),
        duration_ms: 1000,
        status_code: Some("OK".to_string()),
        environment: Some("test".to_string()),
        ..Default::default()
    };

    // A message payload shaped like the ones ingestion writes. The message queries apply
    // MESSAGE_CONTENT_FILTER, so without content on some spans and not others the row sets would
    // be trivially equal and prove nothing about the filter.
    let messages = |text: &str| {
        Some(
            serde_json::json!([{
                "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
                "content": {"role": "user", "content": text}
            }])
            .to_string(),
        )
    };

    let generation = |mut s: NormalizedSpan, input: i64, output: i64, cost: f64| {
        s.observation_type = Some(ObservationType::Generation);
        s.span_category = Some(SpanCategory::LLM);
        s.gen_ai_system = Some("bedrock".to_string());
        s.gen_ai_request_model = Some("claude-haiku".to_string());
        s.gen_ai_response_model = Some("claude-haiku".to_string());
        s.gen_ai_usage_input_tokens = input;
        s.gen_ai_usage_output_tokens = output;
        s.gen_ai_usage_total_tokens = input + output;
        s.gen_ai_usage_cache_read_tokens = 3;
        s.gen_ai_usage_cache_write_tokens = 4;
        s.gen_ai_usage_reasoning_tokens = 5;
        s.gen_ai_cost_input = cost;
        s.gen_ai_cost_output = cost * 2.0;
        s.gen_ai_cost_cache_read = 0.000_001;
        s.gen_ai_cost_cache_write = 0.000_002;
        s.gen_ai_cost_reasoning = 0.000_003;
        s.gen_ai_cost_total = cost * 3.0 + 0.000_006;
        s
    };

    vec![
        // trace-a: root with two generation children, tags spread across spans.
        NormalizedSpan {
            session_id: Some("session-1".to_string()),
            user_id: Some("user-1".to_string()),
            tags: vec!["alpha".to_string(), "shared".to_string()],
            metadata: Some(r#"{"kind":"root"}"#.to_string()),
            input_preview: Some("root input".to_string()),
            output_preview: Some("root output".to_string()),
            observation_type: Some(ObservationType::Agent),
            ..base("trace-a", "a-root", "agent", 0)
        },
        generation(
            NormalizedSpan {
                parent_span_id: Some("a-root".to_string()),
                session_id: Some("session-1".to_string()),
                tags: vec!["beta".to_string(), "shared".to_string()],
                input_preview: Some("child one input".to_string()),
                output_preview: Some("child one output".to_string()),
                messages: messages("first turn"),
                tool_names: Some(r#"["get_weather"]"#.to_string()),
                ..base("trace-a", "a-gen-1", "generation", 1)
            },
            100,
            10,
            0.001,
        ),
        generation(
            NormalizedSpan {
                parent_span_id: Some("a-root".to_string()),
                session_id: Some("session-1".to_string()),
                tags: vec!["gamma".to_string()],
                output_preview: Some("child two output".to_string()),
                messages: messages("second turn"),
                ..base("trace-a", "a-gen-2", "generation", 2)
            },
            200,
            20,
            0.002,
        ),
        // trace-b: same session, single generation root.
        generation(
            NormalizedSpan {
                session_id: Some("session-1".to_string()),
                user_id: Some("user-1".to_string()),
                tags: vec!["alpha".to_string()],
                input_preview: Some("b input".to_string()),
                output_preview: Some("b output".to_string()),
                messages: messages("second trace of the session"),
                ..base("trace-b", "b-root", "generation", 10)
            },
            50,
            5,
            0.0005,
        ),
        // trace-c: no root span - trace_name must fall back to the earliest named span.
        generation(
            NormalizedSpan {
                parent_span_id: Some("c-missing-root".to_string()),
                session_id: Some("session-2".to_string()),
                input_preview: Some("c early input".to_string()),
                ..base("trace-c", "c-child-1", "earliest-named", 20)
            },
            7,
            8,
            0.000_7,
        ),
        NormalizedSpan {
            parent_span_id: Some("c-missing-root".to_string()),
            session_id: Some("session-2".to_string()),
            output_preview: Some("c later output".to_string()),
            ..base("trace-c", "c-child-2", "later-named", 21)
        },
        // trace-d: no session, no generation, error status, no tags. Carries the raw OTLP span,
        // because the event and link reads extract from that JSON and would otherwise compare two
        // empty lists.
        NormalizedSpan {
            raw_span: Some(
                serde_json::json!({
                    "attributes": {"custom.attribute": "value"},
                    "resource": {"attributes": {"service.name": "parity"}},
                    "events": [
                        {"timestamp": "2025-01-01T00:00:00Z", "name": "exception",
                         "attributes": {"exception.type": "ValueError"}},
                        {"timestamp": "2025-01-01T00:00:01Z", "name": "retry",
                         "attributes": {"attempt": 2}}
                    ],
                    "links": [
                        {"trace_id": "trace-a", "span_id": "a-root",
                         "attributes": {"link.kind": "follows"}}
                    ]
                })
                .to_string(),
            ),
            status_code: Some("ERROR".to_string()),
            status_message: Some("boom".to_string()),
            exception_type: Some("ValueError".to_string()),
            exception_message: Some("boom".to_string()),
            observation_type: Some(ObservationType::Tool),
            span_category: Some(SpanCategory::Tool),
            ..base("trace-d", "d-root", "tool", 30)
        },
    ]
}

// ============================================================================
// Field-by-field descriptions
// ============================================================================
// Compared as text rather than with PartialEq: a mismatch has to say *which* column disagreed,
// and floats need a fixed precision so the two dialects' rounding does not read as a defect.

fn f(value: f64) -> String {
    format!("{value:.9}")
}

/// JSON with keys sorted, so a dialect that reorders an object's members while extracting it from
/// the raw span is not reported as a content difference.
fn canonical_json(raw: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(value) => canonical_value(&value),
        // Not JSON at all: compare verbatim rather than silently normalising it away.
        Err(_) => raw.to_string(),
    }
}

fn canonical_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut pairs: Vec<_> = map.iter().collect();
            pairs.sort_by_key(|(k, _)| *k);
            let inner: Vec<String> = pairs
                .iter()
                .map(|(k, v)| format!("{k}:{}", canonical_value(v)))
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        serde_json::Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(canonical_value).collect();
            format!("[{}]", inner.join(","))
        }
        other => other.to_string(),
    }
}

fn describe_trace(t: &TraceRow) -> String {
    let mut tags = t.tags.clone();
    tags.sort();
    format!(
        "trace_id={} name={:?} start={} end={:?} duration={:?} session={:?} user={:?} env={:?} \
         spans={} tokens=[{},{},{},{},{},{}] costs=[{},{},{},{},{},{}] tags={:?} \
         observations={} metadata={:?} input={:?} output={:?} error={}",
        t.trace_id,
        t.trace_name,
        t.start_time.timestamp_micros(),
        t.end_time.map(|e| e.timestamp_micros()),
        t.duration_ms,
        t.session_id,
        t.user_id,
        t.environment,
        t.span_count,
        t.input_tokens,
        t.output_tokens,
        t.total_tokens,
        t.cache_read_tokens,
        t.cache_write_tokens,
        t.reasoning_tokens,
        f(t.input_cost),
        f(t.output_cost),
        f(t.cache_read_cost),
        f(t.cache_write_cost),
        f(t.reasoning_cost),
        f(t.total_cost),
        tags,
        t.observation_count,
        t.metadata,
        t.input_preview,
        t.output_preview,
        t.has_error,
    )
}

fn describe_session(s: &SessionRow) -> String {
    format!(
        "session_id={} user={:?} env={:?} start={} end={:?} traces={} spans={} observations={} \
         tokens=[{},{},{},{},{},{}] costs=[{},{},{},{},{},{}]",
        s.session_id,
        s.user_id,
        s.environment,
        s.start_time.timestamp_micros(),
        s.end_time.map(|e| e.timestamp_micros()),
        s.trace_count,
        s.span_count,
        s.observation_count,
        s.input_tokens,
        s.output_tokens,
        s.total_tokens,
        s.cache_read_tokens,
        s.cache_write_tokens,
        s.reasoning_tokens,
        f(s.input_cost),
        f(s.output_cost),
        f(s.cache_read_cost),
        f(s.cache_write_cost),
        f(s.reasoning_cost),
        f(s.total_cost),
    )
}

fn describe_span(s: &SpanRow) -> String {
    format!(
        "span_id={} trace={} parent={:?} name={:?} kind={:?} category={:?} observation={:?} \
         framework={:?} status={:?} start={} end={:?} duration={:?} env={:?} \
         tokens=[{},{},{}] cost_total={} input={:?} output={:?}",
        s.span_id,
        s.trace_id,
        s.parent_span_id,
        s.span_name,
        s.span_kind,
        s.span_category,
        s.observation_type,
        s.framework,
        s.status_code,
        s.timestamp_start.timestamp_micros(),
        s.timestamp_end.map(|e| e.timestamp_micros()),
        s.duration_ms,
        s.environment,
        s.gen_ai_usage_input_tokens,
        s.gen_ai_usage_output_tokens,
        s.gen_ai_usage_total_tokens,
        f(s.gen_ai_cost_total),
        s.input_preview,
        s.output_preview,
    )
}

/// A message row, field by field. `messages_json` is compared in full: it is the input the SideML
/// pipeline parses, so a single dropped event changes what users see.
fn describe_message_row(r: &MessageSpanRow) -> String {
    format!(
        "span={} trace={} parent={:?} start={} end={:?} model={:?} provider={:?} status={:?} \
         exception={:?}/{:?} tokens=[{},{},{}] cost={} observation={:?} session={:?} \
         messages={} tools={} tool_names={}",
        r.span_id,
        r.trace_id,
        r.parent_span_id,
        r.span_timestamp.timestamp_micros(),
        r.span_end_timestamp.map(|e| e.timestamp_micros()),
        r.model,
        r.provider,
        r.status_code,
        r.exception_type,
        r.exception_message,
        r.input_tokens,
        r.output_tokens,
        r.total_tokens,
        f(r.cost_total),
        r.observation_type,
        r.session_id,
        r.messages_json,
        r.tool_definitions_json,
        r.tool_names_json,
    )
}

// ============================================================================
// Harness
// ============================================================================

async fn duckdb_backend() -> (tempfile::TempDir, Arc<DuckdbService>) {
    let temp = tempfile::TempDir::new().expect("temp dir");
    tokio::fs::create_dir_all(temp.path().join("duckdb"))
        .await
        .expect("duckdb dir");
    let storage = AppStorage::init_for_test(temp.path().to_path_buf());
    let service = DuckdbService::init(&storage).await.expect("duckdb init");
    (temp, Arc::new(service))
}

/// Connects to the ClickHouse named by [`URL_ENV`] in a database of its own, so a run cannot
/// collide with a developer's real data or with a concurrent run.
async fn clickhouse_backend(url: &str, database: &str) -> Arc<ClickhouseService> {
    let user = std::env::var(USER_ENV).ok();
    let password = std::env::var(PASSWORD_ENV).ok();

    // The database has to exist before the client binds to it, and `init` only creates tables.
    let mut bootstrap = clickhouse::Client::default().with_url(url);
    if let Some(ref user) = user {
        bootstrap = bootstrap.with_user(user);
    }
    if let Some(ref password) = password {
        bootstrap = bootstrap.with_password(password);
    }
    bootstrap
        .query(&format!("DROP DATABASE IF EXISTS {database}"))
        .execute()
        .await
        .expect("drop test database");
    bootstrap
        .query(&format!("CREATE DATABASE {database}"))
        .execute()
        .await
        .expect("create test database");

    let config = ClickhouseConfig {
        url: url.to_string(),
        database: database.to_string(),
        user,
        password,
        timeout_secs: 30,
        compression: false,
        // Fire-and-forget batching would let a read run before its own write landed.
        async_insert: false,
        wait_for_async_insert: true,
        cluster: None,
        distributed: false,
    };
    Arc::new(
        ClickhouseService::init(&config)
            .await
            .expect("clickhouse init"),
    )
}

fn trace_params() -> ListTracesParams {
    ListTracesParams {
        project_id: PROJECT.to_string(),
        page: 1,
        limit: 50,
        // Half the fixture is deliberately non-GenAI; excluding it would skip the
        // all-defaults path where tokens must read 0 rather than NULL.
        include_nongenai: true,
        ..Default::default()
    }
}

/// Sorted so an ordering difference between the backends is reported as its own failure rather
/// than smeared across every row.
fn sorted(mut described: Vec<String>) -> Vec<String> {
    described.sort();
    described
}

#[tokio::test]
async fn clickhouse_matches_duckdb_on_every_read() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!(
            "clickhouse parity: skipped - set {URL_ENV} to a ClickHouse HTTP endpoint \
             (or run `make test-clickhouse`)"
        );
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity").await;

    let spans = fixture_spans();
    duck.insert_spans(spans.clone())
        .await
        .expect("duckdb insert");
    ch.insert_spans(spans.clone())
        .await
        .expect("clickhouse insert");

    // --- traces list -------------------------------------------------------
    let (duck_traces, duck_total) = duck
        .list_traces(&trace_params())
        .await
        .expect("duckdb traces");
    let (ch_traces, ch_total) = ch
        .list_traces(&trace_params())
        .await
        .expect("clickhouse traces");
    assert_eq!(
        duck_total, ch_total,
        "trace total count differs: duckdb={duck_total} clickhouse={ch_total}"
    );
    assert!(!duck_traces.is_empty(), "fixture produced no traces");
    assert_eq!(
        sorted(duck_traces.iter().map(describe_trace).collect()),
        sorted(ch_traces.iter().map(describe_trace).collect()),
        "list_traces differs between backends"
    );
    // Ordering is part of the contract, not just membership.
    assert_eq!(
        duck_traces
            .iter()
            .map(|t| t.trace_id.clone())
            .collect::<Vec<_>>(),
        ch_traces
            .iter()
            .map(|t| t.trace_id.clone())
            .collect::<Vec<_>>(),
        "list_traces returns the same traces in a different order"
    );

    // --- single trace ------------------------------------------------------
    for trace_id in ["trace-a", "trace-b", "trace-c", "trace-d"] {
        let d = duck
            .get_trace(PROJECT, trace_id)
            .await
            .expect("duckdb get_trace");
        let c = ch
            .get_trace(PROJECT, trace_id)
            .await
            .expect("clickhouse get_trace");
        match (d, c) {
            (Some(d), Some(c)) => assert_eq!(
                describe_trace(&d),
                describe_trace(&c),
                "get_trace({trace_id}) differs between backends"
            ),
            (d, c) => panic!(
                "get_trace({trace_id}) presence differs: duckdb={} clickhouse={}",
                d.is_some(),
                c.is_some()
            ),
        }
    }

    // A trace list row and a single-trace fetch must agree with each other too: the two
    // projections were copied, so they could drift within one backend.
    let listed = duck_traces
        .iter()
        .find(|t| t.trace_id == "trace-a")
        .expect("trace-a in list");
    let fetched = ch
        .get_trace(PROJECT, "trace-a")
        .await
        .expect("clickhouse get_trace")
        .expect("trace-a exists");
    assert_eq!(
        describe_trace(listed),
        describe_trace(&fetched),
        "the trace list and single-trace projections disagree"
    );

    // --- spans -------------------------------------------------------------
    let span_params = ListSpansParams {
        project_id: PROJECT.to_string(),
        page: 1,
        limit: 50,
        ..Default::default()
    };
    let (duck_spans, duck_span_total) = duck.list_spans(&span_params).await.expect("duckdb spans");
    let (ch_spans, ch_span_total) = ch.list_spans(&span_params).await.expect("clickhouse spans");
    assert_eq!(
        duck_span_total, ch_span_total,
        "span total count differs: duckdb={duck_span_total} clickhouse={ch_span_total}"
    );
    assert_eq!(
        sorted(duck_spans.iter().map(describe_span).collect()),
        sorted(ch_spans.iter().map(describe_span).collect()),
        "list_spans differs between backends"
    );

    for trace_id in ["trace-a", "trace-c"] {
        let d = duck
            .get_spans_for_trace(PROJECT, trace_id)
            .await
            .expect("duckdb spans for trace");
        let c = ch
            .get_spans_for_trace(PROJECT, trace_id)
            .await
            .expect("clickhouse spans for trace");
        assert_eq!(
            d.iter().map(describe_span).collect::<Vec<_>>(),
            c.iter().map(describe_span).collect::<Vec<_>>(),
            "get_spans_for_trace({trace_id}) differs between backends"
        );
    }

    // --- sessions ----------------------------------------------------------
    let session_params = ListSessionsParams {
        project_id: PROJECT.to_string(),
        page: 1,
        limit: 50,
        ..Default::default()
    };
    let (duck_sessions, duck_session_total) = duck
        .list_sessions(&session_params)
        .await
        .expect("duckdb sessions");
    let (ch_sessions, ch_session_total) = ch
        .list_sessions(&session_params)
        .await
        .expect("clickhouse sessions");
    assert_eq!(
        duck_session_total, ch_session_total,
        "session total count differs: duckdb={duck_session_total} clickhouse={ch_session_total}"
    );
    assert_eq!(
        sorted(duck_sessions.iter().map(describe_session).collect()),
        sorted(ch_sessions.iter().map(describe_session).collect()),
        "list_sessions differs between backends"
    );

    for session_id in ["session-1", "session-2"] {
        let d = duck
            .get_session(PROJECT, session_id)
            .await
            .expect("duckdb get_session");
        let c = ch
            .get_session(PROJECT, session_id)
            .await
            .expect("clickhouse get_session");
        match (d, c) {
            (Some(d), Some(c)) => assert_eq!(
                describe_session(&d),
                describe_session(&c),
                "get_session({session_id}) differs between backends"
            ),
            (d, c) => panic!(
                "get_session({session_id}) presence differs: duckdb={} clickhouse={}",
                d.is_some(),
                c.is_some()
            ),
        }

        let d = duck
            .get_traces_for_session(PROJECT, session_id)
            .await
            .expect("duckdb traces for session");
        let c = ch
            .get_traces_for_session(PROJECT, session_id)
            .await
            .expect("clickhouse traces for session");
        assert_eq!(
            d.iter().map(describe_trace).collect::<Vec<_>>(),
            c.iter().map(describe_trace).collect::<Vec<_>>(),
            "get_traces_for_session({session_id}) differs between backends"
        );

        let mut d = duck
            .get_trace_ids_for_sessions(PROJECT, &[session_id.to_string()])
            .await
            .expect("duckdb trace ids");
        let mut c = ch
            .get_trace_ids_for_sessions(PROJECT, &[session_id.to_string()])
            .await
            .expect("clickhouse trace ids");
        d.sort();
        c.sort();
        assert_eq!(
            d, c,
            "get_trace_ids_for_sessions({session_id}) differs between backends"
        );
    }

    // --- message rows ------------------------------------------------------
    // The rows every messages endpoint feeds to the SideML pipeline. The goldens prove the
    // pipeline is right; they say nothing about whether this backend hands it the same rows, and
    // the two dialects build these queries separately - including the content filter and the
    // ordering the pipeline's tie-breaks depend on.
    let message_scopes = [
        (
            "span",
            MessageQueryParams {
                project_id: PROJECT.to_string(),
                span_id: Some("a-gen-1".to_string()),
                trace_id: Some("trace-a".to_string()),
                ..Default::default()
            },
        ),
        (
            "trace",
            MessageQueryParams {
                project_id: PROJECT.to_string(),
                trace_id: Some("trace-a".to_string()),
                ..Default::default()
            },
        ),
        (
            "session",
            MessageQueryParams {
                project_id: PROJECT.to_string(),
                session_id: Some("session-1".to_string()),
                ..Default::default()
            },
        ),
        (
            "trace with no messages",
            MessageQueryParams {
                project_id: PROJECT.to_string(),
                trace_id: Some("trace-d".to_string()),
                ..Default::default()
            },
        ),
    ];
    for (label, params) in message_scopes {
        let d = duck.get_messages(&params).await.expect("duckdb messages");
        let c = ch.get_messages(&params).await.expect("clickhouse messages");
        // An empty answer on both sides is equal and proves nothing, so the fixture is required
        // to produce rows where it is meant to - and none where the content filter should bite.
        match label {
            "trace with no messages" => assert!(
                d.rows.iter().all(|r| r.messages_json == "[]"),
                "the no-message trace must not be carrying message content"
            ),
            _ => assert!(
                !d.rows.is_empty(),
                "get_messages({label}) returned nothing, so this comparison is vacuous"
            ),
        }
        // Compared in order: the pipeline's dedup and history detection walk rows in the order
        // the query returns them, so two backends agreeing on the set but not the sequence can
        // still produce different feeds.
        assert_eq!(
            d.rows.iter().map(describe_message_row).collect::<Vec<_>>(),
            c.rows.iter().map(describe_message_row).collect::<Vec<_>>(),
            "get_messages({label}) differs between backends"
        );
    }

    // --- filter options ----------------------------------------------------
    // The tag options feed the UI's filter dropdown, and its counts come from the same tags
    // column the trace projection unions.
    let describe_options = |rows: Vec<crate::data::traits::FilterOptionRow>| {
        let mut described: Vec<String> = rows
            .into_iter()
            .map(|r| format!("{}={}", r.value, r.count))
            .collect();
        described.sort();
        described
    };
    let d = duck
        .get_trace_tags_options(PROJECT, None, None)
        .await
        .expect("duckdb tags");
    let c = ch
        .get_trace_tags_options(PROJECT, None, None)
        .await
        .expect("clickhouse tags");
    assert_eq!(
        describe_options(d),
        describe_options(c),
        "get_trace_tags_options differs between backends"
    );

    // --- single span, events, links, bulk counts ---------------------------
    for (trace_id, span_id) in [("trace-a", "a-root"), ("trace-d", "d-root")] {
        let d = duck
            .get_span(PROJECT, trace_id, span_id)
            .await
            .expect("duckdb get_span");
        let c = ch
            .get_span(PROJECT, trace_id, span_id)
            .await
            .expect("clickhouse get_span");
        match (d, c) {
            (Some(d), Some(c)) => assert_eq!(
                describe_span(&d),
                describe_span(&c),
                "get_span({span_id}) differs between backends"
            ),
            (d, c) => panic!(
                "get_span({span_id}) presence differs: duckdb={} clickhouse={}",
                d.is_some(),
                c.is_some()
            ),
        }

        // Events and links are extracted from the raw OTLP JSON by two different sets of JSON
        // functions, which is exactly where two dialects drift.
        let d = duck
            .get_events_for_span(PROJECT, trace_id, span_id)
            .await
            .expect("duckdb events");
        let c = ch
            .get_events_for_span(PROJECT, trace_id, span_id)
            .await
            .expect("clickhouse events");
        let describe_event = |e: &crate::data::types::EventRow| {
            format!(
                "span={} index={} time={} name={:?} attributes={:?}",
                e.span_id,
                e.event_index,
                e.event_time.timestamp_micros(),
                e.event_name,
                e.attributes.as_deref().map(canonical_json),
            )
        };
        assert_eq!(
            d.iter().map(describe_event).collect::<Vec<_>>(),
            c.iter().map(describe_event).collect::<Vec<_>>(),
            "get_events_for_span({span_id}) differs between backends"
        );

        let d = duck
            .get_links_for_span(PROJECT, trace_id, span_id)
            .await
            .expect("duckdb links");
        let c = ch
            .get_links_for_span(PROJECT, trace_id, span_id)
            .await
            .expect("clickhouse links");
        let describe_link = |l: &crate::data::types::LinkRow| {
            format!(
                "span={} linked={}/{} attributes={:?}",
                l.span_id,
                l.linked_trace_id,
                l.linked_span_id,
                l.attributes.as_deref().map(canonical_json),
            )
        };
        assert_eq!(
            d.iter().map(describe_link).collect::<Vec<_>>(),
            c.iter().map(describe_link).collect::<Vec<_>>(),
            "get_links_for_span({span_id}) differs between backends"
        );
    }

    // The span with raw OTLP must actually produce events and links, or the loop above compares
    // two empty lists and reports success.
    let events = duck
        .get_events_for_span(PROJECT, "trace-d", "d-root")
        .await
        .expect("duckdb events");
    assert_eq!(events.len(), 2, "the fixture's events were not read back");
    let links = duck
        .get_links_for_span(PROJECT, "trace-d", "d-root")
        .await
        .expect("duckdb links");
    assert_eq!(links.len(), 1, "the fixture's links were not read back");

    let span_keys: Vec<(String, String)> = spans
        .iter()
        .map(|s| (s.trace_id.clone(), s.span_id.clone()))
        .collect();
    let d = duck
        .get_span_counts_bulk(PROJECT, &span_keys)
        .await
        .expect("duckdb counts");
    let c = ch
        .get_span_counts_bulk(PROJECT, &span_keys)
        .await
        .expect("clickhouse counts");
    let describe_counts = |m: &std::collections::HashMap<(String, String), _>| {
        let mut described: Vec<String> = m
            .iter()
            .map(
                |((trace, span), counts): (&(String, String), &crate::data::types::SpanCounts)| {
                    format!(
                        "{trace}/{span}=events:{},links:{}",
                        counts.event_count, counts.link_count
                    )
                },
            )
            .collect();
        described.sort();
        described
    };
    assert_eq!(
        describe_counts(&d),
        describe_counts(&c),
        "get_span_counts_bulk differs between backends"
    );

    // --- project feed ------------------------------------------------------
    // The feed endpoints page with a (ingested_at, span_id) cursor, whose SQL is written twice.
    let feed_params = crate::data::types::FeedSpansParams {
        project_id: PROJECT.to_string(),
        limit: 50,
        cursor: None,
        start_time: None,
        end_time: None,
        is_observation: None,
    };
    let d = duck
        .get_feed_spans(&feed_params)
        .await
        .expect("duckdb feed spans");
    let c = ch
        .get_feed_spans(&feed_params)
        .await
        .expect("clickhouse feed spans");
    assert!(!d.is_empty(), "the feed returned nothing to compare");
    assert_eq!(
        d.iter().map(describe_span).collect::<Vec<_>>(),
        c.iter().map(describe_span).collect::<Vec<_>>(),
        "get_feed_spans differs between backends"
    );

    let feed_messages = crate::data::types::FeedMessagesParams {
        project_id: PROJECT.to_string(),
        limit: 50,
        cursor: None,
        start_time: None,
        end_time: None,
    };
    let d = duck
        .get_project_messages(&feed_messages)
        .await
        .expect("duckdb project messages");
    let c = ch
        .get_project_messages(&feed_messages)
        .await
        .expect("clickhouse project messages");
    assert!(
        !d.rows.is_empty(),
        "the project message feed returned nothing to compare"
    );
    assert_eq!(
        d.rows.iter().map(describe_message_row).collect::<Vec<_>>(),
        c.rows.iter().map(describe_message_row).collect::<Vec<_>>(),
        "get_project_messages differs between backends"
    );

    // --- filter options, all three scopes ----------------------------------
    // Every option's count is shown in the UI next to it, so an approximate count on one backend
    // and an exact one on the other means the same project reports different numbers.
    let describe_option_map =
        |m: std::collections::HashMap<String, Vec<crate::data::traits::FilterOptionRow>>| {
            let mut described: Vec<String> = m
                .into_iter()
                .map(|(column, rows)| {
                    let mut values: Vec<String> = rows
                        .iter()
                        .map(|r| format!("{}={}", r.value, r.count))
                        .collect();
                    values.sort();
                    format!("{column}: {}", values.join(","))
                })
                .collect();
            described.sort();
            described
        };

    // The trace list exposes view column names, which the repositories map to span columns.
    let trace_columns: Vec<String> = crate::data::types::TRACE_FILTER_OPTION_COLUMNS
        .iter()
        .map(|(view_column, _)| view_column.to_string())
        .collect();
    let d = duck
        .get_trace_filter_options(PROJECT, &trace_columns, None, None)
        .await
        .expect("duckdb trace options");
    let c = ch
        .get_trace_filter_options(PROJECT, &trace_columns, None, None)
        .await
        .expect("clickhouse trace options");
    assert_eq!(
        describe_option_map(d),
        describe_option_map(c),
        "get_trace_filter_options differs between backends"
    );

    let span_columns: Vec<String> = crate::data::types::SPAN_FILTER_OPTION_COLUMNS
        .iter()
        .map(|c| c.to_string())
        .collect();
    for observations_only in [false, true] {
        let d = duck
            .get_span_filter_options(PROJECT, &span_columns, None, None, observations_only)
            .await
            .expect("duckdb span options");
        let c = ch
            .get_span_filter_options(PROJECT, &span_columns, None, None, observations_only)
            .await
            .expect("clickhouse span options");
        assert_eq!(
            describe_option_map(d),
            describe_option_map(c),
            "get_span_filter_options(observations_only={observations_only}) differs"
        );
    }

    let session_columns: Vec<String> = crate::data::types::SESSION_FILTER_OPTION_COLUMNS
        .iter()
        .map(|c| c.to_string())
        .collect();
    let d = duck
        .get_session_filter_options(PROJECT, &session_columns, None, None)
        .await
        .expect("duckdb session options");
    let c = ch
        .get_session_filter_options(PROJECT, &session_columns, None, None)
        .await
        .expect("clickhouse session options");
    assert_eq!(
        describe_option_map(d),
        describe_option_map(c),
        "get_session_filter_options differs between backends"
    );

    // --- project span counts -----------------------------------------------
    let d = duck
        .count_spans_by_project(&[PROJECT.to_string()])
        .await
        .expect("duckdb project counts");
    let c = ch
        .count_spans_by_project(&[PROJECT.to_string()])
        .await
        .expect("clickhouse project counts");
    assert_eq!(
        d.get(PROJECT),
        c.get(PROJECT),
        "count_spans_by_project differs between backends"
    );
    assert_eq!(
        d.get(PROJECT),
        Some(&(spans.len() as u64)),
        "the project span count does not match what was inserted"
    );

    // --- stats -------------------------------------------------------------
    let stats_params = crate::data::types::StatsParams {
        project_id: PROJECT.to_string(),
        from_timestamp: ts(-3600),
        to_timestamp: ts(3600),
        timezone: None,
    };
    let d = duck
        .get_project_stats(&stats_params)
        .await
        .expect("duckdb stats");
    let c = ch
        .get_project_stats(&stats_params)
        .await
        .expect("clickhouse stats");
    assert_eq!(
        format!("{d:?}"),
        format!("{c:?}"),
        "get_project_stats differs between backends"
    );
}

/// Deleting must remove the same rows on both backends.
///
/// A dialect difference here is the worst kind: the user asks for data to be gone, one backend
/// obliges and the other keeps it, and the API answers 204 either way. ClickHouse deletes through
/// an asynchronous mutation, so the test waits for the rows to actually disappear instead of
/// reading a count - which is also why the count itself is not compared (see
/// `AnalyticsRepository::delete_traces`).
#[tokio::test]
async fn deleting_removes_the_same_rows_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_delete").await;

    let spans = fixture_spans();
    duck.insert_spans(spans.clone())
        .await
        .expect("duckdb insert");
    ch.insert_spans(spans.clone())
        .await
        .expect("clickhouse insert");

    /// Span ids still present, sorted, from whichever backend.
    async fn remaining(repo: &impl AnalyticsRepository) -> Vec<String> {
        let params = ListSpansParams {
            project_id: PROJECT.to_string(),
            page: 1,
            limit: 200,
            ..Default::default()
        };
        let (rows, _) = repo.list_spans(&params).await.expect("list spans");
        let mut ids: Vec<String> = rows.into_iter().map(|r| r.span_id).collect();
        ids.sort();
        ids
    }

    /// ClickHouse mutations are asynchronous, so poll until the expectation holds rather than
    /// sleeping a guessed interval or trusting the returned count.
    async fn settle(repo: &impl AnalyticsRepository, expected: &[String]) -> Vec<String> {
        for _ in 0..100 {
            let actual = remaining(repo).await;
            if actual == expected {
                return actual;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        remaining(repo).await
    }

    assert_eq!(
        remaining(&duck).await,
        remaining(&ch).await,
        "the two backends disagree before anything was deleted"
    );

    // A whole trace, including the spans that only belong to it through their trace id.
    duck.delete_traces(PROJECT, &["trace-a".to_string()])
        .await
        .expect("duckdb delete trace");
    ch.delete_traces(PROJECT, &["trace-a".to_string()])
        .await
        .expect("clickhouse delete trace");
    let after_duck = remaining(&duck).await;
    assert!(
        !after_duck.iter().any(|id| id.starts_with("a-")),
        "deleting trace-a left its spans behind: {after_duck:?}"
    );
    assert_eq!(
        settle(&ch, &after_duck).await,
        after_duck,
        "delete_traces removed different rows on the two backends"
    );

    // One span out of a trace, leaving its siblings.
    let pair = [("trace-c".to_string(), "c-child-1".to_string())];
    duck.delete_spans(PROJECT, &pair)
        .await
        .expect("duckdb delete span");
    ch.delete_spans(PROJECT, &pair)
        .await
        .expect("clickhouse delete span");
    let after_duck = remaining(&duck).await;
    assert!(
        after_duck.contains(&"c-child-2".to_string())
            && !after_duck.contains(&"c-child-1".to_string()),
        "deleting one span took the wrong rows: {after_duck:?}"
    );
    assert_eq!(
        settle(&ch, &after_duck).await,
        after_duck,
        "delete_spans removed different rows on the two backends"
    );

    // A session, which spans several traces.
    duck.delete_sessions(PROJECT, &["session-2".to_string()])
        .await
        .expect("duckdb delete session");
    ch.delete_sessions(PROJECT, &["session-2".to_string()])
        .await
        .expect("clickhouse delete session");
    let after_duck = remaining(&duck).await;
    assert_eq!(
        settle(&ch, &after_duck).await,
        after_duck,
        "delete_sessions removed different rows on the two backends"
    );

    // Everything that is left.
    duck.delete_project_data(PROJECT)
        .await
        .expect("duckdb delete project");
    ch.delete_project_data(PROJECT)
        .await
        .expect("clickhouse delete project");
    assert!(
        remaining(&duck).await.is_empty(),
        "delete_project_data left rows behind on duckdb"
    );
    assert!(
        settle(&ch, &[]).await.is_empty(),
        "delete_project_data left rows behind on clickhouse"
    );
}

/// The fixture has to actually exercise the cases the parity assertions exist for; a fixture that
/// silently lost its no-root trace or its tag spread would let both backends agree on nothing.
#[test]
fn the_fixture_covers_the_cases_parity_depends_on() {
    let spans = fixture_spans();

    let trace_c: Vec<_> = spans.iter().filter(|s| s.trace_id == "trace-c").collect();
    assert!(
        !trace_c.is_empty() && trace_c.iter().all(|s| s.parent_span_id.is_some()),
        "trace-c must have no root span, so trace_name exercises the earliest-named fallback"
    );

    let trace_a_tags: Vec<&String> = spans
        .iter()
        .filter(|s| s.trace_id == "trace-a")
        .flat_map(|s| s.tags.iter())
        .collect();
    let distinct = trace_a_tags
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    assert!(
        trace_a_tags.len() > distinct,
        "trace-a must spread overlapping tags across spans, so the tags union is tested"
    );
    assert!(
        spans
            .iter()
            .filter(|s| s.trace_id == "trace-a")
            .filter(|s| s.observation_type == Some(ObservationType::Generation))
            .count()
            > 1,
        "trace-a must hold several generation spans, so token dedup is tested"
    );

    assert!(
        spans.iter().any(|s| s.session_id.is_none()),
        "one trace must have no session, so the no-session path is tested"
    );
    assert!(
        spans
            .iter()
            .any(|s| s.status_code.as_deref() == Some("ERROR")),
        "one span must carry an error, so has_error is tested"
    );
    assert!(
        spans
            .iter()
            .any(|s| s.observation_type != Some(ObservationType::Generation)
                && s.gen_ai_usage_total_tokens == 0),
        "one span must have no tokens, so the zero-not-null path is tested"
    );

    let sessions: std::collections::BTreeSet<_> =
        spans.iter().filter_map(|s| s.session_id.as_ref()).collect();
    assert!(
        sessions.len() >= 2,
        "at least two sessions, so session aggregation is not trivially one group"
    );
    let session_1_traces: std::collections::BTreeSet<_> = spans
        .iter()
        .filter(|s| s.session_id.as_deref() == Some("session-1"))
        .map(|s| &s.trace_id)
        .collect();
    assert!(
        session_1_traces.len() >= 2,
        "session-1 must span several traces, so session totals cross trace boundaries"
    );
}
