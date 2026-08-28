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
//! So: insert one span set into both backends, call the analytics reads on both, and require the
//! answers to match. DuckDB is the reference because it is the default backend and its behaviour
//! is what the goldens and the UI were built against.
//!
//! Covered: trace list and single trace, span list, spans for a trace, single span, events, links,
//! bulk span counts, session list and single session, traces and trace ids for a session, message
//! rows for span/trace/session, the project feed's span and message pages, filter options for all
//! three scopes, tag options, project span counts, project stats, the four delete paths, and -
//! per filter variant, since each is rendered by its own arm - pagination, sorting, time bounds
//! and the advanced filters on the trace, span and session lists.
//!
//! Not covered, and worth knowing before trusting a green run:
//!
//! - metric ingestion and reads.
//! - anything that only appears at scale or with data this fixture does not have: a trace of
//!   thousands of spans, top-N truncation in stats, several models or frameworks in one project.
//! - re-ingested or duplicated spans, and spans sharing a timestamp exactly, where the two
//!   dialects' `argMin`/`FIRST` tie-breaking could differ.
//! - sub-second timestamp handling: the fixture uses whole seconds.
//! - rows old enough for the ClickHouse TTL to reap, distributed (sharded) mode, and async
//!   inserts - all three are configurations this single-node test cannot enter.
//! - timezone and DST behaviour in the stats bucketing.
//! - `ingested_at`, which is the server clock at write time and so differs by design.
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
use crate::data::duckdb::filters::{DatetimeOp, Filter, NullOp, NumberOp, OptionsOp, StringOp};
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
                // A quote and a non-ASCII character: ClickHouse returns tag values as raw JSON, so
                // this is the tag that fails when they are unquoted by trimming rather than decoded.
                tags: vec!["gamma".to_string(), r#"say "café""#.to_string()],
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
        // trace-e: a plain span with no observation type, so the include_nongenai filter has
        // something to exclude. Without it that filter matched every trace and the case asserted
        // nothing.
        //
        // It also starts at exactly the same instant as trace-f, which is the tie the pagination
        // ordering has to break: with no tiebreak, two rows with the same sort value have no
        // defined order between them and one can appear on two pages or on none.
        NormalizedSpan {
            ..base("trace-e", "e-root", "plain-span", 40)
        },
        NormalizedSpan {
            ..base("trace-f", "f-root", "plain-span", 40)
        },
        // trace-g: GenAI attributes on a plain span with no observation type, which is what
        // transport-level instrumentation produces. It is a GenAI trace and the "GenAI only" filter
        // has to keep it - ClickHouse required an observation and dropped it, while DuckDB accepted
        // it, so the same project showed a different trace list per backend.
        NormalizedSpan {
            gen_ai_system: Some("bedrock".to_string()),
            gen_ai_request_model: Some("claude-haiku".to_string()),
            ..base("trace-g", "g-root", "http-post", 50)
        },
        // trace-i: the session id is on the root span only, which is how several frameworks record
        // it - the session queries have a CTE for exactly that reason. The session's totals have to
        // include the child, which carries the tokens and no session id of its own.
        NormalizedSpan {
            session_id: Some("session-3".to_string()),
            observation_type: Some(ObservationType::Agent),
            ..base("trace-i", "i-root", "agent", 70)
        },
        generation(
            NormalizedSpan {
                parent_span_id: Some("i-root".to_string()),
                ..base("trace-i", "i-gen", "generation", 71)
            },
            300,
            30,
            0.003,
        ),
        // trace-h: qualifies through token usage alone - no observation type, no provider, no
        // model. Instrumentation that reports only usage looks like this, and a predicate that
        // checks the provider and the request model dropped it.
        NormalizedSpan {
            gen_ai_usage_input_tokens: 11,
            gen_ai_usage_output_tokens: 2,
            gen_ai_usage_total_tokens: 13,
            ..base("trace-h", "h-root", "usage-only", 60)
        },
        // trace-j: qualifies through *cost* alone, which is what OpenInference's `llm.cost.*`
        // produces - the cost is reported directly, not derived from usage this span carries. The
        // GenAI predicate listed tokens and not cost, so this span was a plain span and vanished
        // from every view that filters to GenAI.
        NormalizedSpan {
            gen_ai_cost_total: 0.004,
            ..base("trace-j", "j-root", "cost-only", 65)
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
    // Every field, because a comparison is only as good as the columns it looks at: the earlier
    // version read 19 of the 38 a SpanRow carries, so half the projection was unchecked.
    // JSON-valued columns go through canonical_json, since the two dialects are free to emit an
    // object's members in different orders while extracting it.
    format!(
        "span_id={} trace={} parent={:?} name={:?} kind={:?} category={:?} observation={:?} \
         framework={:?} status={:?} start={} end={:?} duration={:?} env={:?} \
         resource_attributes={:?} session={:?} user={:?} system={:?} request_model={:?} \
         agent_name={:?} finish_reasons={:?} tokens=[{},{},{},{},{},{}] \
         costs=[{},{},{},{},{},{}] usage_details={:?} metadata={:?} attributes={:?} \
         input={:?} output={:?} raw_span={:?}",
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
        s.resource_attributes.as_deref().map(canonical_json),
        s.session_id,
        s.user_id,
        s.gen_ai_system,
        s.gen_ai_request_model,
        s.gen_ai_agent_name,
        s.gen_ai_finish_reasons,
        s.gen_ai_usage_input_tokens,
        s.gen_ai_usage_output_tokens,
        s.gen_ai_usage_total_tokens,
        s.gen_ai_usage_cache_read_tokens,
        s.gen_ai_usage_cache_write_tokens,
        s.gen_ai_usage_reasoning_tokens,
        f(s.gen_ai_cost_input),
        f(s.gen_ai_cost_output),
        f(s.gen_ai_cost_cache_read),
        f(s.gen_ai_cost_cache_write),
        f(s.gen_ai_cost_reasoning),
        f(s.gen_ai_cost_total),
        s.gen_ai_usage_details.as_deref().map(canonical_json),
        s.metadata.as_deref().map(canonical_json),
        s.attributes.as_deref().map(canonical_json),
        s.input_preview,
        s.output_preview,
        s.raw_span.as_deref().map(canonical_json),
    )
    // ingested_at is deliberately absent: it defaults to the server clock at write time, so the
    // two backends record different values for the same span by design.
}

/// A message row, field by field. `messages_json` is compared in full: it is the input the SideML
/// pipeline parses, so a single dropped event changes what users see.
fn describe_message_row(r: &MessageSpanRow) -> String {
    format!(
        "span={} trace={} parent={:?} start={} end={:?} model={:?} provider={:?} status={:?} \
         exception={:?}/{:?}/{:?} tokens=[{},{},{}] cost={} observation={:?} session={:?} \
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
        r.exception_stacktrace,
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

    // The session whose id is on the root span only: its totals must include the child span, which
    // carries the tokens. Restricting the aggregation to rows that name the session counted the
    // root alone and reported a session with no tokens at all.
    let root_only = duck
        .get_session(PROJECT, "session-3")
        .await
        .expect("duckdb get_session")
        .expect("session-3 exists");
    assert_eq!(
        root_only.span_count, 2,
        "the session's span count excluded the child span"
    );
    assert_eq!(
        root_only.total_tokens, 330,
        "the session's tokens excluded the child span, which is the span that has them"
    );
    assert_eq!(
        describe_session(&root_only),
        describe_session(
            &ch.get_session(PROJECT, "session-3")
                .await
                .expect("clickhouse get_session")
                .expect("session-3 exists")
        ),
        "get_session(session-3) differs between backends"
    );

    // And the same session as the *list* reports it. The single-session query resolves the
    // session's traces first, so it sees the child; the list grouped rows by the id they carry,
    // which is a different set - so the row a user sees in the list and the page they open from it
    // disagreed.
    let listed = duck_sessions
        .iter()
        .find(|s| s.session_id == "session-3")
        .expect("session-3 in the list");
    assert_eq!(
        describe_session(listed),
        describe_session(&root_only),
        "the session list and the single-session view disagree"
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
        (
            // Several traces at once, which is how the project feed loads the traces on a page in
            // full before narrowing the reconstruction back to the page.
            "many traces",
            MessageQueryParams {
                project_id: PROJECT.to_string(),
                trace_ids: Some(vec!["trace-a".to_string(), "trace-b".to_string()]),
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
            // The content filter keeps a row with no messages when it carries an error, so this
            // scope has exactly one row and it is empty of messages. Asserting only "every row is
            // empty" passed for zero rows, which is the case that would mean the filter had
            // dropped the error row entirely.
            "trace with no messages" => {
                assert_eq!(
                    d.rows.len(),
                    1,
                    "the error-only trace must still return its row: {:?}",
                    d.rows.iter().map(|r| &r.span_id).collect::<Vec<_>>()
                );
                assert_eq!(d.rows[0].messages_json, "[]");
                assert_eq!(d.rows[0].status_code.as_deref(), Some("ERROR"));
            }
            // Both traces must come back, not just the first: an IN over several ids is a new
            // clause in both dialects, and one that silently matched only one id would still look
            // non-empty.
            "many traces" => {
                let traces: std::collections::BTreeSet<&str> =
                    d.rows.iter().map(|r| r.trace_id.as_str()).collect();
                assert_eq!(
                    traces.into_iter().collect::<Vec<_>>(),
                    vec!["trace-a", "trace-b"],
                    "the multi-trace query did not return both traces"
                );
            }
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
    let described = describe_options(d);
    assert_eq!(
        described,
        describe_options(c),
        "get_trace_tags_options differs between backends"
    );
    // The fixture's tags, with the trace counts they carry: alpha is on trace-a and trace-b, the
    // rest on trace-a alone. Two empty lists would otherwise pass.
    //
    // `say "café"` is the one that matters here: ClickHouse returns tag values as raw JSON, and
    // unquoting them by trimming the outer quotes offered this tag as `say \"caf\u00e9\"` - a value
    // the filter could never match. Both backends must produce the decoded string.
    assert_eq!(
        described,
        vec!["alpha=2", "beta=1", "gamma=1", "say \"café\"=1", "shared=1"],
        "the tag options do not match the fixture's tags"
    );

    // --- pagination, sorting, filters and time bounds ----------------------
    // Paged over traces and sessions as well as spans, and over both cursor feeds, because the
    // claim was "pagination" while only the span list actually asked for a second page - so the
    // total-order fix those queries needed could have regressed unnoticed.
    let mut trace_pages: Vec<Vec<String>> = Vec::new();
    // Enough pages to cover the fixture whatever its size, so adding a trace does not silently
    // stop the coverage assertion below from meaning anything.
    let trace_page_count = (duck_traces.len() as u32).div_ceil(2);
    for page in 1..=trace_page_count {
        let params = ListTracesParams {
            page,
            limit: 2,
            ..trace_params()
        };
        let (d, d_total) = duck.list_traces(&params).await.expect("duckdb trace page");
        let (c, c_total) = ch
            .list_traces(&params)
            .await
            .expect("clickhouse trace page");
        assert_eq!(d_total, c_total, "trace page {page}: totals differ");
        assert_eq!(
            d.iter().map(describe_trace).collect::<Vec<_>>(),
            c.iter().map(describe_trace).collect::<Vec<_>>(),
            "trace page {page} differs between backends"
        );
        trace_pages.push(d.iter().map(|t| t.trace_id.clone()).collect());
    }
    let paged_traces: Vec<&String> = trace_pages.iter().flatten().collect();
    let distinct_traces: std::collections::BTreeSet<&&String> = paged_traces.iter().collect();
    assert_eq!(
        paged_traces.len(),
        distinct_traces.len(),
        "a trace appeared on two pages: {trace_pages:?}"
    );
    assert_eq!(
        distinct_traces.len(),
        duck_traces.len(),
        "paging did not cover every trace: {trace_pages:?}"
    );

    let mut session_pages: Vec<String> = Vec::new();
    for page in 1..=2 {
        let params = ListSessionsParams {
            project_id: PROJECT.to_string(),
            page,
            limit: 1,
            ..Default::default()
        };
        let (d, d_total) = duck
            .list_sessions(&params)
            .await
            .expect("duckdb session page");
        let (c, c_total) = ch
            .list_sessions(&params)
            .await
            .expect("clickhouse session page");
        assert_eq!(d_total, c_total, "session page {page}: totals differ");
        assert_eq!(
            d.iter().map(describe_session).collect::<Vec<_>>(),
            c.iter().map(describe_session).collect::<Vec<_>>(),
            "session page {page} differs between backends"
        );
        assert_eq!(d.len(), 1, "session page {page} should hold one session");
        session_pages.push(d[0].session_id.clone());
    }
    // Two backends both returning page one twice would satisfy equality; the pages have to be
    // different sessions and together cover the fixture's two.
    assert_eq!(
        session_pages
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        2,
        "the two session pages returned the same session: {session_pages:?}"
    );

    // A single unpaginated unfiltered call exercises none of the offset arithmetic, ORDER BY
    // translation or predicate building, which is where two dialects have the most room to differ.
    let mut page_ids: Vec<Vec<String>> = Vec::new();
    // Derived from the fixture, so adding a span does not quietly leave the last one unpaged and
    // turn the coverage assertion below into nothing.
    let span_page_count = (spans.len() as u32).div_ceil(3);
    for page in 1..=span_page_count {
        let params = ListSpansParams {
            project_id: PROJECT.to_string(),
            page,
            limit: 3,
            order_by: Some(crate::api::types::OrderBy {
                column: "timestamp_start".to_string(),
                direction: crate::api::types::OrderDirection::Asc,
            }),
            ..Default::default()
        };
        let (d, d_total) = duck.list_spans(&params).await.expect("duckdb page");
        let (c, c_total) = ch.list_spans(&params).await.expect("clickhouse page");
        assert_eq!(d_total, c_total, "page {page}: totals differ");
        assert_eq!(
            d.iter().map(describe_span).collect::<Vec<_>>(),
            c.iter().map(describe_span).collect::<Vec<_>>(),
            "page {page} of list_spans differs between backends"
        );
        page_ids.push(d.iter().map(|s| s.span_id.clone()).collect());
    }
    // Pages must be disjoint and cover the fixture, or the offset arithmetic is wrong in a way
    // both backends could share.
    let paged: Vec<&String> = page_ids.iter().flatten().collect();
    let distinct: std::collections::BTreeSet<&&String> = paged.iter().collect();
    assert_eq!(
        paged.len(),
        distinct.len(),
        "paging returned the same span twice: {page_ids:?}"
    );
    assert_eq!(
        paged.len(),
        spans.len(),
        "paging did not cover every span: {page_ids:?}"
    );

    for (label, params) in [
        (
            "sorted by cost desc",
            ListTracesParams {
                order_by: Some(crate::api::types::OrderBy {
                    column: "total_cost".to_string(),
                    direction: crate::api::types::OrderDirection::Desc,
                }),
                ..trace_params()
            },
        ),
        (
            "filtered by environment",
            ListTracesParams {
                environment: Some(vec!["test".to_string()]),
                ..trace_params()
            },
        ),
        (
            "filtered by user",
            ListTracesParams {
                user_id: Some("user-1".to_string()),
                ..trace_params()
            },
        ),
        // One case per filter variant: each is rendered by its own arm, in a dialect where the
        // wrong shape either raises or silently matches everything.
        (
            "filtered by a token count",
            ListTracesParams {
                filters: vec![Filter::Number {
                    column: "total_tokens".to_string(),
                    operator: NumberOp::Gt,
                    value: 100.0,
                }],
                ..trace_params()
            },
        ),
        (
            "filtered by a decimal cost",
            ListTracesParams {
                filters: vec![Filter::Number {
                    column: "total_cost".to_string(),
                    operator: NumberOp::Gte,
                    value: 0.001,
                }],
                ..trace_params()
            },
        ),
        (
            // The Name filter the UI offers is a select over the displayed names, so it has to
            // match the displayed name: "agent" must return the traces shown as "agent" and not
            // every trace that merely contains an agent span.
            "filtered by the displayed name",
            ListTracesParams {
                filters: vec![Filter::StringOptions {
                    column: "trace_name".to_string(),
                    operator: OptionsOp::AnyOf,
                    value: vec!["agent".to_string()],
                }],
                ..trace_params()
            },
        ),
        (
            // A name filter combined with another one. The name has to be computed from the trace's
            // whole span set, as the projection computes it - not from the rows the model filter
            // left behind, which for a root-agent/generation-child trace is the child, so the row
            // came back labelled "agent" after being selected as "generation".
            "filtered by name and model together",
            ListTracesParams {
                filters: vec![
                    Filter::StringOptions {
                        column: "trace_name".to_string(),
                        operator: OptionsOp::AnyOf,
                        value: vec!["agent".to_string()],
                    },
                    Filter::String {
                        column: "gen_ai_request_model".to_string(),
                        operator: StringOp::Eq,
                        value: "claude-haiku".to_string(),
                    },
                ],
                ..trace_params()
            },
        ),
        (
            "filtered by a name substring",
            ListTracesParams {
                filters: vec![Filter::String {
                    column: "trace_name".to_string(),
                    operator: StringOp::Contains,
                    value: "gen".to_string(),
                }],
                ..trace_params()
            },
        ),
        (
            "filtered by an exact model",
            ListTracesParams {
                filters: vec![Filter::String {
                    column: "gen_ai_request_model".to_string(),
                    operator: StringOp::Eq,
                    value: "claude-haiku".to_string(),
                }],
                ..trace_params()
            },
        ),
        (
            "filtered by tags, any of",
            ListTracesParams {
                filters: vec![Filter::StringOptions {
                    column: "tags".to_string(),
                    operator: OptionsOp::AnyOf,
                    value: vec!["beta".to_string(), "gamma".to_string()],
                }],
                ..trace_params()
            },
        ),
        (
            "filtered by tags, none of",
            ListTracesParams {
                filters: vec![Filter::StringOptions {
                    column: "tags".to_string(),
                    operator: OptionsOp::NoneOf,
                    value: vec!["alpha".to_string()],
                }],
                ..trace_params()
            },
        ),
        (
            "filtered by environment options",
            ListTracesParams {
                filters: vec![Filter::StringOptions {
                    column: "environment".to_string(),
                    operator: OptionsOp::AnyOf,
                    value: vec!["test".to_string()],
                }],
                ..trace_params()
            },
        ),
        (
            // "None of" over a nullable column. A trace with no user id at all is not one of the
            // listed users, so it belongs in the result - which is what the complement form
            // (`trace_id NOT IN (traces whose user is listed)`) gives. Rendering the negation
            // directly instead made both dialects evaluate `NULL NOT IN (...)` to NULL and drop
            // those traces silently, and made a trace with two users match because one of them was
            // someone else. This is the case that pins the quantifier, because every other filtered
            // column is populated for every row.
            "filtered by none of a nullable column",
            ListTracesParams {
                filters: vec![Filter::StringOptions {
                    column: "user_id".to_string(),
                    operator: OptionsOp::NoneOf,
                    value: vec!["someone-else".to_string()],
                }],
                ..trace_params()
            },
        ),
        (
            // Against the session the row *displays*. trace-i records its session on the root span
            // and nothing on its generation child, so a row-level check called it session-less
            // while the list showed session-3.
            "filtered by a null session",
            ListTracesParams {
                filters: vec![Filter::Null {
                    column: "session_id".to_string(),
                    operator: NullOp::IsNull,
                }],
                ..trace_params()
            },
        ),
        (
            "filtered by a datetime",
            ListTracesParams {
                filters: vec![Filter::Datetime {
                    column: "start_time".to_string(),
                    operator: DatetimeOp::Gte,
                    value: ts(10).to_rfc3339(),
                }],
                ..trace_params()
            },
        ),
        (
            "time bounded",
            ListTracesParams {
                from_timestamp: Some(ts(-60)),
                to_timestamp: Some(ts(15)),
                ..trace_params()
            },
        ),
        (
            "genai only",
            ListTracesParams {
                include_nongenai: false,
                ..trace_params()
            },
        ),
        (
            // trace-a's spans hold 110 and 220 tokens and the list displays their sum, 330. A
            // filter applied to one span row hid it from `> 250` because neither span reaches the
            // threshold, and returned it for `< 150` because one span is under - the row visible on
            // screen contradicted the filter that was supposed to have selected it.
            "filtered by a token total no single span reaches",
            ListTracesParams {
                filters: vec![Filter::Number {
                    column: "total_tokens".to_string(),
                    operator: NumberOp::Gt,
                    value: 250.0,
                }],
                ..trace_params()
            },
        ),
        (
            // trace-i carries its session id on the root span and its tokens on the generation
            // child. ANDed on one span row, the two conditions ask for a span with both, which no
            // span in that trace has, so the trace disappeared from a list that showed it under
            // either filter alone.
            "filtered by a session and a token count together",
            ListTracesParams {
                filters: vec![
                    Filter::String {
                        column: "session_id".to_string(),
                        operator: StringOp::Eq,
                        value: "session-3".to_string(),
                    },
                    Filter::Number {
                        column: "total_tokens".to_string(),
                        operator: NumberOp::Gt,
                        value: 100.0,
                    },
                ],
                ..trace_params()
            },
        ),
    ] {
        let (d, d_total) = duck.list_traces(&params).await.expect("duckdb traces");
        let (c, c_total) = ch.list_traces(&params).await.expect("clickhouse traces");
        assert_eq!(d_total, c_total, "{label}: totals differ");
        assert_eq!(
            d.iter().map(describe_trace).collect::<Vec<_>>(),
            c.iter().map(describe_trace).collect::<Vec<_>>(),
            "list_traces {label} differs between backends"
        );
        // What makes each case non-vacuous, stated per case. `d.len() <= total` let a filter that
        // was dropped entirely pass, which is the failure being guarded against; but a sort
        // returns everything by design, and two of these filters do match every trace in this
        // fixture, so "strict subset" cannot be the rule for all of them.
        assert!(!d.is_empty(), "{label} selected no traces at all");
        match label {
            // Order is the observable, so compare it against the default ordering.
            "sorted by cost desc" => assert_ne!(
                d.iter().map(|t| &t.trace_id).collect::<Vec<_>>(),
                duck_traces.iter().map(|t| &t.trace_id).collect::<Vec<_>>(),
                "the sort returned the default order, so it exercises nothing"
            ),
            // Every span in the fixture carries this environment, so matching all of them is
            // correct here and the parity comparison is what this case contributes.
            "filtered by environment" | "filtered by environment options" => assert_eq!(
                d.len(),
                duck_traces.len(),
                "the fixture changed: this filter no longer matches every trace"
            ),
            // Excludes the two plain traces but must keep trace-g, whose GenAI attributes sit on a
            // span with no observation type.
            "genai only" => {
                let kept: Vec<&String> = d.iter().map(|t| &t.trace_id).collect();
                // trace-g qualifies through attributes, trace-h through token usage, trace-j
                // through cost alone - the three shapes instrumentation produces without an
                // observation type. Removing any one clause from the predicate fails here.
                for expected in ["trace-g", "trace-h", "trace-j"] {
                    assert!(
                        kept.contains(&&expected.to_string()),
                        "the GenAI filter dropped {expected}, whose GenAI data is on a plain span: \
                         {kept:?}"
                    );
                }
                // Visible is not enough: trace-j's cost has to be *counted*. Token and cost
                // aggregation admitted a row only when it reported tokens, so a span reporting cost
                // alone was listed with a cost of zero - and then sorted, filtered and totalled as
                // free.
                let cost_only = d
                    .iter()
                    .find(|t| t.trace_id == "trace-j")
                    .expect("trace-j is in the list");
                assert!(
                    (cost_only.total_cost - 0.004).abs() < 1e-9,
                    "the cost-only trace reports {} rather than the 0.004 it was billed",
                    cost_only.total_cost
                );
                assert!(
                    !kept.contains(&&"trace-e".to_string())
                        && !kept.contains(&&"trace-f".to_string()),
                    "the GenAI filter kept a trace with no GenAI data at all: {kept:?}"
                );
            }
            "filtered by name and model together" => {
                let names: Vec<Option<&String>> = d.iter().map(|t| t.trace_name.as_ref()).collect();
                assert!(
                    names.iter().all(|n| n.map(String::as_str) == Some("agent")),
                    "combining the filters returned a trace displayed under another name: {names:?}"
                );
                assert_eq!(
                    d.len(),
                    2,
                    "both agent-named traces have a span carrying the model: {names:?}"
                );
            }
            "filtered by the displayed name" => {
                let names: Vec<Option<&String>> = d.iter().map(|t| t.trace_name.as_ref()).collect();
                assert!(
                    names.iter().all(|n| n.map(String::as_str) == Some("agent")),
                    "the name filter returned traces displayed under another name: {names:?}"
                );
                assert_eq!(
                    d.len(),
                    2,
                    "the fixture has two traces displayed as agent: {names:?}"
                );
            }
            // Every returned row must show a total that satisfies the filter, and the trace whose
            // total only exists as a sum must be among them.
            "filtered by a token total no single span reaches" => {
                let kept: Vec<&String> = d.iter().map(|t| &t.trace_id).collect();
                assert!(
                    kept.contains(&&"trace-a".to_string()),
                    "the trace displaying 330 tokens across two spans of 110 and 220 is missing: \
                     {kept:?}"
                );
                for t in &d {
                    assert!(
                        t.total_tokens > 250,
                        "trace {} is displayed with {} tokens but was selected by `> 250`",
                        t.trace_id,
                        t.total_tokens
                    );
                }
            }
            // trace-a and trace-b hold a session on every span, trace-i on the root only, trace-c
            // on both of its spans; only trace-d and the plain traces display none.
            "filtered by a null session" => {
                let kept: Vec<&String> = d.iter().map(|t| &t.trace_id).collect();
                assert!(
                    !kept.contains(&&"trace-i".to_string()),
                    "trace-i displays session-3 on its root span, so it is not session-less: \
                     {kept:?}"
                );
                for t in &d {
                    assert!(
                        t.session_id.is_none(),
                        "trace {} is displayed under session {:?} but matched `is null`",
                        t.trace_id,
                        t.session_id
                    );
                }
            }
            "filtered by a session and a token count together" => {
                let kept: Vec<&String> = d.iter().map(|t| &t.trace_id).collect();
                assert_eq!(
                    kept,
                    vec![&"trace-i".to_string()],
                    "the session is on the root span and the tokens on its child; both describe \
                     the trace: {kept:?}"
                );
                assert!(
                    d[0].total_tokens > 100,
                    "the row was selected by a token filter it does not satisfy: {}",
                    d[0].total_tokens
                );
            }
            _ => assert!(
                d.len() < duck_traces.len(),
                "{label} selected all {} traces, so a dropped filter would pass",
                d.len()
            ),
        }
    }

    // The span and session lists take filters through their own column mappers, so the wiring is
    // exercised per query rather than assumed from the trace case.
    for (label, params) in [
        (
            "span filtered by model",
            ListSpansParams {
                project_id: PROJECT.to_string(),
                page: 1,
                limit: 50,
                filters: vec![Filter::String {
                    column: "gen_ai_request_model".to_string(),
                    operator: StringOp::Eq,
                    value: "claude-haiku".to_string(),
                }],
                ..Default::default()
            },
        ),
        (
            "span filtered by token count",
            ListSpansParams {
                project_id: PROJECT.to_string(),
                page: 1,
                limit: 50,
                filters: vec![Filter::Number {
                    column: "gen_ai_usage_total_tokens".to_string(),
                    operator: NumberOp::Gte,
                    value: 100.0,
                }],
                ..Default::default()
            },
        ),
    ] {
        let (d, d_total) = duck.list_spans(&params).await.expect("duckdb spans");
        let (c, c_total) = ch.list_spans(&params).await.expect("clickhouse spans");
        assert_eq!(d_total, c_total, "{label}: totals differ");
        assert_eq!(
            d.iter().map(describe_span).collect::<Vec<_>>(),
            c.iter().map(describe_span).collect::<Vec<_>>(),
            "list_spans {label} differs between backends"
        );
        assert!(
            !d.is_empty() && d.len() < spans.len(),
            "{label} selected {} of {} spans, so it exercises nothing",
            d.len(),
            spans.len()
        );
    }

    let session_filtered = ListSessionsParams {
        project_id: PROJECT.to_string(),
        page: 1,
        limit: 50,
        filters: vec![Filter::String {
            column: "user_id".to_string(),
            operator: StringOp::Eq,
            value: "user-1".to_string(),
        }],
        ..Default::default()
    };
    let (d, d_total) = duck
        .list_sessions(&session_filtered)
        .await
        .expect("duckdb sessions");
    let (c, c_total) = ch
        .list_sessions(&session_filtered)
        .await
        .expect("clickhouse sessions");
    assert_eq!(d_total, c_total, "filtered session totals differ");
    assert_eq!(
        d.iter().map(describe_session).collect::<Vec<_>>(),
        c.iter().map(describe_session).collect::<Vec<_>>(),
        "filtered list_sessions differs between backends"
    );
    assert_eq!(
        d.len(),
        1,
        "the session filter selected {} sessions, so it exercises nothing",
        d.len()
    );
    // session-1 spans trace-a and trace-b, and only trace-b carries user-1. The filter selects the
    // session through that trace; it must not shrink the session to it - selection and membership
    // are separate questions, and one predicate for both listed the session with one trace and
    // trace-b's tokens alone while opening it showed both.
    assert_eq!(
        d[0].trace_count, 2,
        "the filtered session lost the trace that does not name its user: {:?}",
        d[0].trace_count
    );
    assert_eq!(
        d[0].total_tokens, 385,
        "the session's tokens must cover both of its traces (110 + 220 + 55), not only the trace \
         the filter matched: {} reported",
        d[0].total_tokens
    );

    // Every column the API accepts as a trace sort must actually sort by it. One that is accepted
    // and unmapped falls through to min_ts, so the list comes back in time order while the UI shows
    // the chosen column as active - which was true of total_tokens.
    for column in crate::data::duckdb::filters::columns::TRACE_SORTABLE {
        let params = ListTracesParams {
            order_by: Some(crate::api::types::OrderBy {
                column: column.to_string(),
                direction: crate::api::types::OrderDirection::Desc,
            }),
            ..trace_params()
        };
        let (d, _) = duck.list_traces(&params).await.expect("duckdb sorted");
        let (c, _) = ch.list_traces(&params).await.expect("clickhouse sorted");
        assert_eq!(
            d.iter().map(describe_trace).collect::<Vec<_>>(),
            c.iter().map(describe_trace).collect::<Vec<_>>(),
            "sorting traces by {column} differs between backends"
        );
        // Descending by the requested column, whatever it is.
        let values: Vec<f64> = d
            .iter()
            .map(|t| match *column {
                "start_time" => t.start_time.timestamp_micros() as f64,
                "end_time" => t.end_time.unwrap_or(t.start_time).timestamp_micros() as f64,
                "duration_ms" => t.duration_ms.unwrap_or(0) as f64,
                "total_tokens" => t.total_tokens as f64,
                "total_cost" => t.total_cost,
                other => panic!("{other} is sortable but this test does not read it"),
            })
            .collect();
        assert!(
            values.windows(2).all(|w| w[0] >= w[1]),
            "sorting traces by {column} descending produced {values:?}"
        );
    }

    for column in crate::data::duckdb::filters::columns::SESSION_SORTABLE {
        let params = ListSessionsParams {
            project_id: PROJECT.to_string(),
            page: 1,
            limit: 50,
            order_by: Some(crate::api::types::OrderBy {
                column: column.to_string(),
                direction: crate::api::types::OrderDirection::Desc,
            }),
            ..Default::default()
        };
        let (d, _) = duck.list_sessions(&params).await.expect("duckdb sorted");
        let (c, _) = ch.list_sessions(&params).await.expect("clickhouse sorted");
        assert_eq!(
            d.iter().map(describe_session).collect::<Vec<_>>(),
            c.iter().map(describe_session).collect::<Vec<_>>(),
            "sorting sessions by {column} differs between backends"
        );
        let values: Vec<f64> = d
            .iter()
            .map(|s| match *column {
                "start_time" => s.start_time.timestamp_micros() as f64,
                "end_time" => s.end_time.unwrap_or(s.start_time).timestamp_micros() as f64,
                "trace_count" => s.trace_count as f64,
                "span_count" => s.span_count as f64,
                "observation_count" => s.observation_count as f64,
                other => panic!("{other} is sortable but this test does not read it"),
            })
            .collect();
        assert!(
            values.windows(2).all(|w| w[0] >= w[1]),
            "sorting sessions by {column} descending produced {values:?}"
        );
    }

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

    // A window whose start falls *inside* a span. The fixture's spans each run for one second, so a
    // window starting half a second after trace-a's first generation began still contains the moment
    // it finished - and a completed response carries its span's end time, so its message belongs in
    // that window. Selecting rows by the span's start dropped it before reconstruction could see it.
    let straddling = crate::data::types::FeedMessagesParams {
        project_id: PROJECT.to_string(),
        limit: 50,
        cursor: None,
        start_time: Some(ts(1) + chrono::Duration::milliseconds(500)),
        end_time: None,
    };
    let d_straddle = duck
        .get_project_messages(&straddling)
        .await
        .expect("duckdb straddling window");
    let c_straddle = ch
        .get_project_messages(&straddling)
        .await
        .expect("clickhouse straddling window");
    assert_eq!(
        d_straddle
            .rows
            .iter()
            .map(describe_message_row)
            .collect::<Vec<_>>(),
        c_straddle
            .rows
            .iter()
            .map(describe_message_row)
            .collect::<Vec<_>>(),
        "a window starting inside a span differs between backends"
    );
    assert!(
        d_straddle.rows.iter().any(|r| r.span_id == "a-gen-1"),
        "the span that began before the window and finished inside it is missing: {:?}",
        d_straddle
            .rows
            .iter()
            .map(|r| &r.span_id)
            .collect::<Vec<_>>()
    );

    // Cursor paging, which is a different mechanism from LIMIT/OFFSET and was only ever called
    // with `cursor: None`.
    let feed_page = |cursor: Option<(i64, String, String)>| crate::data::types::FeedSpansParams {
        project_id: PROJECT.to_string(),
        limit: 3,
        cursor,
        start_time: None,
        end_time: None,
        is_observation: None,
    };
    let d_first = duck
        .get_feed_spans(&feed_page(None))
        .await
        .expect("duckdb feed page 1");
    let c_first = ch
        .get_feed_spans(&feed_page(None))
        .await
        .expect("clickhouse feed page 1");
    assert_eq!(
        d_first.iter().map(describe_span).collect::<Vec<_>>(),
        c_first.iter().map(describe_span).collect::<Vec<_>>(),
        "the first cursor page of the span feed differs between backends"
    );
    assert_eq!(d_first.len(), 3, "the feed's first page should be full");
    // Each backend's cursor comes from its own page: the cursor carries `ingested_at`, which is
    // the server clock at write time and therefore differs between the two for the same span. A
    // cursor from one applied to the other selects nothing, which says nothing about either.
    let duck_cursor = d_first.last().map(|s| {
        (
            s.ingested_at.timestamp_micros(),
            s.span_id.clone(),
            s.trace_id.clone(),
        )
    });
    let ch_cursor = c_first.last().map(|s| {
        (
            s.ingested_at.timestamp_micros(),
            s.span_id.clone(),
            s.trace_id.clone(),
        )
    });
    let d_second = duck
        .get_feed_spans(&feed_page(duck_cursor))
        .await
        .expect("duckdb feed page 2");
    let c_second = ch
        .get_feed_spans(&feed_page(ch_cursor))
        .await
        .expect("clickhouse feed page 2");
    assert_eq!(
        d_second.iter().map(describe_span).collect::<Vec<_>>(),
        c_second.iter().map(describe_span).collect::<Vec<_>>(),
        "the second cursor page of the span feed differs between backends"
    );
    let first_ids: std::collections::BTreeSet<&String> =
        d_first.iter().map(|s| &s.span_id).collect();
    assert!(
        !d_second.is_empty() && d_second.iter().all(|s| !first_ids.contains(&s.span_id)),
        "the cursor returned rows the first page already had"
    );

    let messages_page =
        |cursor: Option<(i64, String, String)>| crate::data::types::FeedMessagesParams {
            project_id: PROJECT.to_string(),
            limit: 2,
            cursor,
            start_time: None,
            end_time: None,
        };
    let d_first = duck
        .get_project_messages(&messages_page(None))
        .await
        .expect("duckdb message feed page 1");
    let c_first = ch
        .get_project_messages(&messages_page(None))
        .await
        .expect("clickhouse message feed page 1");
    assert_eq!(
        d_first
            .rows
            .iter()
            .map(describe_message_row)
            .collect::<Vec<_>>(),
        c_first
            .rows
            .iter()
            .map(describe_message_row)
            .collect::<Vec<_>>(),
        "the first cursor page of the message feed differs between backends"
    );
    // Per-backend cursor again, for the same reason.
    let duck_cursor = d_first.rows.last().map(|r| {
        (
            r.ingested_at.timestamp_micros(),
            r.span_id.clone(),
            r.trace_id.clone(),
        )
    });
    let ch_cursor = c_first.rows.last().map(|r| {
        (
            r.ingested_at.timestamp_micros(),
            r.span_id.clone(),
            r.trace_id.clone(),
        )
    });
    let d_second = duck
        .get_project_messages(&messages_page(duck_cursor))
        .await
        .expect("duckdb message feed page 2");
    let c_second = ch
        .get_project_messages(&messages_page(ch_cursor))
        .await
        .expect("clickhouse message feed page 2");
    assert_eq!(
        d_second
            .rows
            .iter()
            .map(describe_message_row)
            .collect::<Vec<_>>(),
        c_second
            .rows
            .iter()
            .map(describe_message_row)
            .collect::<Vec<_>>(),
        "the second cursor page of the message feed differs between backends"
    );
    // Equality alone is satisfied by two empty pages, or by two backends both ignoring the cursor
    // and repeating page one.
    let first_ids: std::collections::BTreeSet<&String> =
        d_first.rows.iter().map(|r| &r.span_id).collect();
    assert!(
        !d_second.rows.is_empty(),
        "the message feed's second page was empty, so the comparison proves nothing"
    );
    assert!(
        d_second
            .rows
            .iter()
            .all(|r| !first_ids.contains(&r.span_id)),
        "the message cursor returned rows the first page already had"
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
    // Compared against the fixture, not only against each other: two backends returning empty maps,
    // or omitting every column asked for, satisfied equality.
    assert_eq!(
        describe_option_map(d),
        describe_option_map(c),
        "get_trace_filter_options differs between backends"
    );
    let d = duck
        .get_trace_filter_options(PROJECT, &trace_columns, None, None)
        .await
        .expect("duckdb trace options");
    let described = describe_option_map(d);
    for expected in [
        // Every span carries this environment, and there are ten traces.
        "environment: test=10",
        // Two sessions, one covering two traces and one covering one.
        "session_id: session-1=2,session-2=1,session-3=1",
        "user_id: user-1=2",
        // The same names the trace list displays, including trace-c's: it has no root span, so
        // its name comes from the earliest named span, exactly as the list's fallback does. Listing
        // root spans only omitted it, and filtering by the name the UI showed returned nothing.
        "trace_name: agent=2,cost-only=1,earliest-named=1,generation=1,http-post=1,plain-span=2,\
         tool=1,usage-only=1",
    ] {
        assert!(
            described.iter().any(|line| line == expected),
            "trace filter options are missing {expected:?}: {described:?}"
        );
    }

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
        let described = describe_option_map(d);
        assert_eq!(
            described,
            describe_option_map(c),
            "get_span_filter_options(observations_only={observations_only}) differs"
        );
        // Against the fixture, not only against each other: two empty maps satisfied equality.
        //
        // "GenAI only" means the same predicate the trace and session lists use, so trace-g's span -
        // GenAI attributes, no observation type, which is what transport-level instrumentation
        // produces - is kept either way. All six spans carrying a model are offered under both
        // settings; a backend that restricted this to observations would report five and hide a span
        // whose trace the trace list shows.
        for (column, value) in [
            ("gen_ai_request_model", "claude-haiku"),
            ("gen_ai_system", "bedrock"),
        ] {
            let expected = format!("{column}: {value}=6");
            assert!(
                described.contains(&expected),
                "span options are missing {expected:?} \
                 (observations_only={observations_only}): {described:?}"
            );
        }

        // And the flag still excludes something: trace-e and trace-f are plain spans with no GenAI
        // data at all. Without this the assertions above would pass for a backend ignoring the flag.
        let names = described
            .iter()
            .find(|d| d.starts_with("span_name: "))
            .expect("span_name options");
        assert_eq!(
            names.contains("plain-span"),
            !observations_only,
            "span_name options with observations_only={observations_only}: {names}"
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
    let described = describe_option_map(d);
    assert_eq!(
        described,
        describe_option_map(c),
        "get_session_filter_options differs between backends"
    );
    // Counted in sessions here, not traces: all three sessions carry the environment, one carries
    // the user. Two empty maps would otherwise pass.
    assert_eq!(
        described,
        vec!["environment: test=3", "user_id: user-1=1"],
        "session options do not match the fixture"
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

    // Each step asserts which ids are gone and which remain, on the reference backend, before
    // comparing. Equality alone is satisfied by two backends that both deleted nothing.

    // One span out of a trace, leaving its sibling.
    let pair = [("trace-c".to_string(), "c-child-1".to_string())];
    duck.delete_spans(PROJECT, &pair)
        .await
        .expect("duckdb delete span");
    ch.delete_spans(PROJECT, &pair)
        .await
        .expect("clickhouse delete span");
    let after_duck = remaining(&duck).await;
    assert!(
        !after_duck.contains(&"c-child-1".to_string())
            && after_duck.contains(&"c-child-2".to_string()),
        "deleting one span took the wrong rows: {after_duck:?}"
    );
    assert_eq!(
        settle(&ch, &after_duck).await,
        after_duck,
        "delete_spans removed different rows on the two backends"
    );

    // A session spanning two traces: session-1 covers trace-a and trace-b, so this must remove
    // spans from both. Deleting session-2 would have touched a single trace, and an assertion that
    // only compared the backends would have passed even if neither deleted anything.
    duck.delete_sessions(PROJECT, &["session-1".to_string()])
        .await
        .expect("duckdb delete session");
    ch.delete_sessions(PROJECT, &["session-1".to_string()])
        .await
        .expect("clickhouse delete session");
    let after_duck = remaining(&duck).await;
    for gone in ["a-root", "a-gen-1", "a-gen-2", "b-root"] {
        assert!(
            !after_duck.contains(&gone.to_string()),
            "deleting session-1 left {gone} behind: {after_duck:?}"
        );
    }
    assert!(
        after_duck.contains(&"c-child-2".to_string()),
        "deleting session-1 took a span from another session: {after_duck:?}"
    );
    assert_eq!(
        settle(&ch, &after_duck).await,
        after_duck,
        "delete_sessions removed different rows on the two backends"
    );

    // A session recorded on the root span only, which is how several frameworks record it. Deleting
    // the rows that *name* the session removes the root and keeps its children - and reports
    // success, leaving spans that no longer belong to any session and so can never be deleted by
    // session again. session-1 above cannot catch it, because the fixture repeats its id on every
    // span; trace-i carries session-3 on its root and nothing on its generation child.
    duck.delete_sessions(PROJECT, &["session-3".to_string()])
        .await
        .expect("duckdb delete root-only session");
    ch.delete_sessions(PROJECT, &["session-3".to_string()])
        .await
        .expect("clickhouse delete root-only session");
    let after_duck = remaining(&duck).await;
    for gone in ["i-root", "i-gen"] {
        assert!(
            !after_duck.contains(&gone.to_string()),
            "deleting the root-only session left {gone} behind: {after_duck:?}"
        );
    }
    assert_eq!(
        settle(&ch, &after_duck).await,
        after_duck,
        "deleting a root-only session removed different rows on the two backends"
    );

    // What is left of a trace, by trace id.
    duck.delete_traces(PROJECT, &["trace-c".to_string()])
        .await
        .expect("duckdb delete trace");
    ch.delete_traces(PROJECT, &["trace-c".to_string()])
        .await
        .expect("clickhouse delete trace");
    let after_duck = remaining(&duck).await;
    assert_eq!(
        after_duck,
        vec![
            "d-root".to_string(),
            "e-root".to_string(),
            "f-root".to_string(),
            "g-root".to_string(),
            "h-root".to_string(),
            "j-root".to_string()
        ],
        "deleting trace-c should leave exactly the unrelated traces"
    );
    assert_eq!(
        settle(&ch, &after_duck).await,
        after_duck,
        "delete_traces removed different rows on the two backends"
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
    assert!(
        spans.iter().any(|s| s.observation_type.is_none()),
        "one span must have no observation type, or the include_nongenai filter excludes nothing"
    );

    let starts: Vec<_> = spans.iter().map(|s| s.timestamp_start).collect();
    let distinct_starts: std::collections::BTreeSet<_> = starts.iter().collect();
    assert!(
        starts.len() > distinct_starts.len(),
        "two spans must start at the same instant, or nothing tests the pagination tiebreak"
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
