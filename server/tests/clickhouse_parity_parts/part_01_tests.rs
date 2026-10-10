use sideseat_ports::traits::{
    AnalyticsMaintenance, AnalyticsRepository, EntityQuery, LogStore, MessageStore, MetricStore,
    SearchIndex, SpanStore, SurvivorReferences,
};
use std::sync::Arc;

use chrono::{DateTime, Datelike, TimeZone, Utc};

use sideseat_adapter_clickhouse::ClickhouseService;
use sideseat_adapter_duckdb::DuckdbService;
use sideseat_core::config::ClickhouseConfig;
use sideseat_core::storage::AppStorage;
use sideseat_ports::filters::{DatetimeOp, Filter, NullOp, NumberOp, OptionsOp, StringOp};

use sideseat_ports::types::{
    AggregationTemporality, FeedSpansParams, ListSessionsParams, ListSpansParams, ListTracesParams,
    MessageQueryParams, MessageSpanRow, MetricType, NormalizedLog, NormalizedMetric,
    NormalizedSpan, ObservationType, ProjectId, RequestContextRows, SearchQuery, SearchRecord,
    SearchSignal, SessionRow, SpanCategory, SpanRow, TraceRow,
};

/// Env var holding the base URL of a ClickHouse HTTP endpoint, e.g. `http://127.0.0.1:8123`.
const URL_ENV: &str = "SIDESEAT_TEST_CLICKHOUSE_URL";
/// Credentials, when the server requires them. Recent official images generate a random password
/// for `default` and reject unauthenticated queries outright.
const USER_ENV: &str = "SIDESEAT_TEST_CLICKHOUSE_USER";
const PASSWORD_ENV: &str = "SIDESEAT_TEST_CLICKHOUSE_PASSWORD";

/// A ClickHouse configured as a **replicated cluster**, which [`URL_ENV`] is not.
///
/// Separate from [`URL_ENV`] because the two are structurally different servers, not two addresses for one:
/// distributed mode needs Keeper, a `remote_servers` entry and `macros`, and a plain server has none of them.
/// `make test-clickhouse-replicated` starts one; the tests that need it skip with a message otherwise, exactly
/// as the single-node ones do.
const REPLICATED_URL_ENV: &str = "SIDESEAT_TEST_CLICKHOUSE_REPLICATED_URL";
/// The cluster name declared in that server's config, so the test and the fixture cannot drift.
const REPLICATED_CLUSTER: &str = "test_cluster";

/// A ClickHouse configured as a **two-shard** cluster.
///
/// Separate from [`REPLICATED_URL_ENV`] because the shard count is the fixture, not a detail: on one shard a
/// read against `otel_spans_local` and a read against the `Distributed` front end return the same rows, so
/// every test passes whichever the code uses. Two of round three's findings were exactly that - the consistency
/// check and the pre-identity metric count both read `_local`, reporting one shard's view as the deployment's -
/// and neither was falsifiable until this existed.
const TWO_SHARD_URL_ENV: &str = "SIDESEAT_TEST_CLICKHOUSE_TWO_SHARD_URL";

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
                // The two derived columns, set on one span of the corpus and left clear on the rest, so a
                // backend that stored either differently is a difference the comparison sees rather than two
                // columns that are empty everywhere.
                request_thread: r#"["parity.thread","session-1"]"#.to_string(),
                span_marks: 0b101,
                request_frame: "parity.frame".to_string(),
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
        // trace-d: no session, no generation, error status, no tags. Two events and one link, as counts:
        // the events and links themselves are rendered from the raw record, not read from a column.
        NormalizedSpan {
            scope_name: Some("opentelemetry.instrumentation.test".to_string()),
            scope_version: Some("1.2.3".to_string()),
            event_count: 2,
            link_count: 1,
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

/// JSON with keys sorted, so a dialect that reorders an object's members while extracting it from a stored
/// JSON column is not reported as a content difference.
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
         session={:?} user={:?} system={:?} request_model={:?} \
         agent_name={:?} finish_reasons={:?} tokens=[{},{},{},{},{},{}] \
         costs=[{},{},{},{},{},{}] usage_details={:?} metadata={:?} \
         input={:?} output={:?} scope_name={:?} scope_version={:?}",
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
        s.input_preview,
        s.output_preview,
        s.scope_name,
        s.scope_version,
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
         messages={} tools={} tool_names={} scope={:?}/{:?} span_name={:?} framework={:?} \
         response={:?}/{:?} params=[{:?},{:?},{:?}] finish={:?} \
         usage=[{},{},{}] cost_split=[{},{}] thread={} marks={} frame={}",
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
        r.scope_name,
        r.scope_version,
        r.span_name,
        r.framework,
        r.response_model,
        r.response_id,
        r.temperature,
        r.top_p,
        r.max_tokens,
        r.finish_reasons,
        r.cache_read_tokens,
        r.cache_write_tokens,
        r.reasoning_tokens,
        f(r.cost_input),
        f(r.cost_output),
        // The two columns a read derives nothing from and a read-time rule depends on: a composed request's
        // thread, and the marks a projection asks about. A backend that stored either differently would feed
        // the pipeline different answers while every other column matched.
        r.request_thread,
        r.span_marks,
        // And the key a detached frame names a request by.
        r.request_frame,
    )
}

// ============================================================================
// Harness
// ============================================================================

async fn duckdb_backend() -> (tempfile::TempDir, sideseat_adapter_duckdb::DuckdbRepository) {
    let temp = tempfile::TempDir::new().expect("temp dir");
    tokio::fs::create_dir_all(temp.path().join("duckdb"))
        .await
        .expect("duckdb dir");
    let storage = AppStorage::init_for_test(temp.path().to_path_buf());
    let service = DuckdbService::init(
        &storage,
        std::sync::Arc::new(sideseat_server::runtime::clock::SystemClock),
    )
    .await
    .expect("duckdb init");
    (
        temp,
        sideseat_adapter_duckdb::DuckdbRepository(Arc::new(service)),
    )
}

/// Connects to the ClickHouse named by [`URL_ENV`] in a database of its own, so a run cannot
/// collide with a developer's real data or with a concurrent run.
/// A bare client on one database, for the statements a repository has no reason to expose.
fn raw_client(url: &str, database: &str) -> clickhouse::Client {
    let mut client = clickhouse::Client::default()
        .with_url(url)
        .with_database(database);
    if let Ok(user) = std::env::var(USER_ENV) {
        client = client.with_user(user);
    }
    if let Ok(password) = std::env::var(PASSWORD_ENV) {
        client = client.with_password(password);
    }
    client
}

async fn clickhouse_backend(
    url: &str,
    database: &str,
) -> sideseat_adapter_clickhouse::ClickhouseRepository {
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
        insert_quorum: 0,
    };
    sideseat_adapter_clickhouse::ClickhouseRepository(Arc::new(
        ClickhouseService::init(
            &config,
            std::sync::Arc::new(sideseat_server::runtime::clock::SystemClock),
        )
        .await
        .expect("clickhouse init"),
    ))
}

/// A service in **distributed** mode against the replicated fixture.
///
/// The single-node helper hardcodes `distributed: false`, which would leave the `ON CLUSTER` path, the
/// `Replicated*` engines and the `Distributed` front tables of schema creation unexercised.
async fn replicated_backend(
    url: &str,
    database: &str,
) -> sideseat_adapter_clickhouse::ClickhouseRepository {
    replicated_backend_at(url, database).await
}

/// The same, named separately so the two-shard tests read as using their own fixture.
async fn replicated_backend_at(
    url: &str,
    database: &str,
) -> sideseat_adapter_clickhouse::ClickhouseRepository {
    let user = std::env::var(USER_ENV).ok();
    let password = std::env::var(PASSWORD_ENV).ok();

    let bootstrap = raw_client_at(url, "default", &user, &password);
    bootstrap
        .query(&format!(
            "DROP DATABASE IF EXISTS {database} ON CLUSTER {REPLICATED_CLUSTER} SYNC"
        ))
        .execute()
        .await
        .expect("drop the test database");
    bootstrap
        .query(&format!(
            "CREATE DATABASE {database} ON CLUSTER {REPLICATED_CLUSTER}"
        ))
        .execute()
        .await
        .expect("create the test database");

    let config = ClickhouseConfig {
        url: url.to_string(),
        database: database.to_string(),
        user,
        password,
        timeout_secs: 30,
        compression: false,
        async_insert: false,
        wait_for_async_insert: true,
        cluster: Some(REPLICATED_CLUSTER.to_string()),
        distributed: true,
        // One replica per shard, so a quorum of two would block every insert forever - which is the
        // legitimate deployment the startup warning exists for rather than refuses.
        insert_quorum: 0,
    };
    sideseat_adapter_clickhouse::ClickhouseRepository(Arc::new(
        ClickhouseService::init(
            &config,
            std::sync::Arc::new(sideseat_server::runtime::clock::SystemClock),
        )
        .await
        .expect("clickhouse init in distributed mode"),
    ))
}

/// A raw client with explicit credentials, so the replicated helper can reach `default` before its own
/// database exists.
fn raw_client_at(
    url: &str,
    database: &str,
    user: &Option<String>,
    password: &Option<String>,
) -> clickhouse::Client {
    let mut client = clickhouse::Client::default()
        .with_url(url)
        .with_database(database);
    if let Some(user) = user {
        client = client.with_user(user);
    }
    if let Some(password) = password {
        client = client.with_password(password);
    }
    client
}

#[tokio::test]
async fn clickhouse_row_policies_are_per_query_and_fail_closed() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let _repository = clickhouse_backend(&url, "sideseat_parity_row_policy").await;
    let raw = raw_client(&url, "sideseat_parity_row_policy");
    let maintenance = raw.clone().with_option(
        sideseat_adapter_clickhouse::schema::TENANT_MAINTENANCE_SETTING,
        "1",
    );

    for (project_id, trace_id) in [("tenant-a", "trace-a"), ("tenant-b", "trace-b")] {
        maintenance
            .query(
                "INSERT INTO otel_spans \
                 (project_id, trace_id, span_id, timestamp_start) \
                 VALUES (?, ?, 'shared-span', now64(6))",
            )
            .bind(project_id)
            .bind(trace_id)
            .execute()
            .await
            .expect("seed a tenant row");
    }

    let policy_tables: Vec<String> = raw
        .query(
            "SELECT table FROM system.row_policies \
             WHERE database = currentDatabase() AND short_name = 'sideseat_tenant_filter' \
             ORDER BY table",
        )
        .fetch_all()
        .await
        .expect("inspect tenant row policies");
    assert_eq!(
        policy_tables,
        vec![
            "otel_logs",
            "otel_metrics",
            "otel_raw",
            "otel_raw_pending",
            "otel_raw_traces",
            "otel_spans",
            "span_partition_anomalies",
        ]
    );

    let tenant = raw.clone().with_option(
        sideseat_adapter_clickhouse::schema::TENANT_PROJECT_SETTING,
        "tenant-a",
    );
    let observed_project: String = tenant
        .query("SELECT getSettingOrDefault('SQL_sideseat_project_id', '')")
        .fetch_one()
        .await
        .expect("read the tenant setting from the query context");
    assert_eq!(observed_project, "tenant-a");
    let seeded_rows: u64 = maintenance
        .query("SELECT count() FROM otel_spans")
        .fetch_one()
        .await
        .expect("verify the maintenance seed");
    assert_eq!(seeded_rows, 2);
    let visible: Vec<String> = tenant
        .query(
            // Deliberately no project predicate: the row policy is the storage backstop.
            "SELECT project_id FROM otel_spans ORDER BY project_id",
        )
        .fetch_all()
        .await
        .expect("tenant-scoped raw read");
    assert_eq!(visible, vec!["tenant-a"]);

    let visible_without_context: Vec<String> = raw
        .query("SELECT project_id FROM otel_spans ORDER BY project_id")
        .fetch_all()
        .await
        .expect("fail-closed raw read");
    assert!(
        visible_without_context.is_empty(),
        "a request without a tenant setting must match no rows"
    );

    let all_rows: u64 = maintenance
        .query("SELECT count() FROM otel_spans")
        .fetch_one()
        .await
        .expect("maintenance read");
    assert_eq!(all_rows, 2);
}

fn trace_params() -> ListTracesParams {
    ListTracesParams {
        project_id: ProjectId::from(PROJECT),
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
