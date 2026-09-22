//! DuckDB schema definitions.
//!
//! Append-only storage with no PRIMARY KEY constraints.
//! Deduplication happens at read time: SpanRow/MessageSpanRow queries
//! are deduped in Rust (DedupAnalyticsRepository), aggregation queries
//! use an inline DEDUP_SPANS subquery.

/// Current schema version
pub const SCHEMA_VERSION: i32 = 6;

/// Complete schema SQL
pub const SCHEMA: &str = r#"
-- Infrastructure: Schema version tracking
CREATE TABLE IF NOT EXISTS schema_version (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    version INTEGER NOT NULL,
    applied_at BIGINT NOT NULL,
    description VARCHAR
);

-- ═══════════════════════════════════════════════════════════════════════════════
-- OTEL spans table: Main table for all OpenTelemetry span data
-- No PRIMARY KEY for append-only ingestion; deduplication at query time
-- ═══════════════════════════════════════════════════════════════════════════════
CREATE TABLE IF NOT EXISTS otel_spans (
    -- ═══════════════════════════════════════════════════════════════════
    -- IDENTITY
    -- ═══════════════════════════════════════════════════════════════════
    project_id          VARCHAR,            -- Tenant isolation
    trace_id            VARCHAR NOT NULL,   -- OTEL trace ID (hex)
    span_id             VARCHAR NOT NULL,   -- OTEL span ID (hex)
    parent_span_id      VARCHAR,            -- Parent span (NULL = root)
    trace_state         VARCHAR,            -- W3C trace state

    -- ═══════════════════════════════════════════════════════════════════
    -- CONTEXT (Session, User, Environment)
    -- ═══════════════════════════════════════════════════════════════════
    session_id          VARCHAR,            -- Conversation/session grouping
    user_id             VARCHAR,            -- End user identifier
    environment         VARCHAR,            -- dev/staging/prod

    -- ═══════════════════════════════════════════════════════════════════
    -- SPAN METADATA
    -- ═══════════════════════════════════════════════════════════════════
    span_name           VARCHAR,            -- Operation name
    span_kind           VARCHAR,            -- OTEL kind: CLIENT, SERVER, INTERNAL, etc.
    status_code         VARCHAR,            -- OK, ERROR, UNSET
    status_message      VARCHAR,            -- Error message if status=ERROR
    exception_type      VARCHAR,            -- Exception class/type name
    exception_message   VARCHAR,            -- Exception message text
    exception_stacktrace VARCHAR,           -- Exception stacktrace

    -- ═══════════════════════════════════════════════════════════════════
    -- CLASSIFICATION (Extracted at ingestion)
    -- ═══════════════════════════════════════════════════════════════════
    span_category       VARCHAR,            -- LLM, Tool, Agent, HTTP, DB, etc.
    observation_type    VARCHAR,            -- Generation, Embedding, Agent, Tool, etc.
    framework           VARCHAR,            -- the producer label detection resolved; a display and filter value

    -- ═══════════════════════════════════════════════════════════════════
    -- TIMING
    -- ═══════════════════════════════════════════════════════════════════
    timestamp_start     TIMESTAMP NOT NULL, -- Span start time (UTC)
    timestamp_end       TIMESTAMP,          -- Span end time (UTC)
    duration_ms         BIGINT,             -- Pre-computed duration
    ingested_at         TIMESTAMP NOT NULL DEFAULT (now()), -- Server receipt time

    -- ═══════════════════════════════════════════════════════════════════
    -- GEN AI: PROVIDER & MODEL
    -- ═══════════════════════════════════════════════════════════════════
    gen_ai_system               VARCHAR,    -- openai, anthropic, bedrock, etc.
    gen_ai_operation_name       VARCHAR,    -- chat, completion, embedding
    gen_ai_request_model        VARCHAR,    -- Requested model ID
    gen_ai_response_model       VARCHAR,    -- Actual model used (may differ)
    gen_ai_response_id          VARCHAR,    -- Provider response ID

    -- ═══════════════════════════════════════════════════════════════════
    -- GEN AI: REQUEST PARAMETERS
    -- ═══════════════════════════════════════════════════════════════════
    gen_ai_temperature          DOUBLE,     -- Sampling temperature
    gen_ai_top_p                DOUBLE,     -- Nucleus sampling
    gen_ai_top_k                BIGINT,     -- Top-k sampling
    gen_ai_max_tokens           BIGINT,     -- Max output tokens requested
    gen_ai_frequency_penalty    DOUBLE,     -- Frequency penalty
    gen_ai_presence_penalty     DOUBLE,     -- Presence penalty
    gen_ai_stop_sequences       VARCHAR,    -- Stop sequences (JSON array)

    -- ═══════════════════════════════════════════════════════════════════
    -- GEN AI: RESPONSE METADATA
    -- ═══════════════════════════════════════════════════════════════════
    gen_ai_finish_reasons       VARCHAR,    -- stop, length, tool_use, etc. (JSON array)

    -- ═══════════════════════════════════════════════════════════════════
    -- GEN AI: AGENT & TOOL
    -- ═══════════════════════════════════════════════════════════════════
    gen_ai_agent_id             VARCHAR,    -- Agent identifier
    gen_ai_agent_name           VARCHAR,    -- Agent display name
    gen_ai_tool_name            VARCHAR,    -- Tool/function name
    gen_ai_tool_call_id         VARCHAR,    -- Tool call correlation ID

    -- ═══════════════════════════════════════════════════════════════════
    -- GEN AI: PERFORMANCE METRICS
    -- ═══════════════════════════════════════════════════════════════════
    gen_ai_server_ttft_ms       BIGINT,     -- Time to first token
    gen_ai_server_request_duration_ms BIGINT, -- Server-side duration

    -- ═══════════════════════════════════════════════════════════════════
    -- GEN AI: TOKEN USAGE (NOT NULL DEFAULT 0)
    -- Zero-default enables SUM() without COALESCE in most queries
    -- ═══════════════════════════════════════════════════════════════════
    gen_ai_usage_input_tokens       BIGINT NOT NULL DEFAULT 0,
    gen_ai_usage_output_tokens      BIGINT NOT NULL DEFAULT 0,
    gen_ai_usage_total_tokens       BIGINT NOT NULL DEFAULT 0,
    gen_ai_usage_cache_read_tokens  BIGINT NOT NULL DEFAULT 0,  -- Anthropic/OpenAI cache
    gen_ai_usage_cache_write_tokens BIGINT NOT NULL DEFAULT 0,
    gen_ai_usage_reasoning_tokens   BIGINT NOT NULL DEFAULT 0,  -- o1/o3/thinking
    gen_ai_usage_details            JSON,   -- Overflow: provider-specific usage

    -- ═══════════════════════════════════════════════════════════════════
    -- GEN AI: COSTS (NOT NULL DEFAULT 0, DECIMAL for precision)
    -- ═══════════════════════════════════════════════════════════════════
    gen_ai_cost_input           DECIMAL(18,6) NOT NULL DEFAULT 0,
    gen_ai_cost_output          DECIMAL(18,6) NOT NULL DEFAULT 0,
    gen_ai_cost_cache_read      DECIMAL(18,6) NOT NULL DEFAULT 0,
    gen_ai_cost_cache_write     DECIMAL(18,6) NOT NULL DEFAULT 0,
    gen_ai_cost_reasoning       DECIMAL(18,6) NOT NULL DEFAULT 0,
    gen_ai_cost_total           DECIMAL(18,6) NOT NULL DEFAULT 0,

    -- ═══════════════════════════════════════════════════════════════════
    -- SEMANTIC CONVENTIONS: HTTP
    -- ═══════════════════════════════════════════════════════════════════
    http_method                 VARCHAR,    -- GET, POST, etc.
    http_url                    VARCHAR,    -- Full URL
    http_status_code            INTEGER,    -- 200, 404, 500, etc.

    -- ═══════════════════════════════════════════════════════════════════
    -- SEMANTIC CONVENTIONS: DATABASE
    -- ═══════════════════════════════════════════════════════════════════
    db_system                   VARCHAR,    -- postgresql, mysql, mongodb
    db_name                     VARCHAR,    -- Database name
    db_operation                VARCHAR,    -- SELECT, INSERT, etc.
    db_statement                VARCHAR,    -- SQL statement (may be truncated)

    -- ═══════════════════════════════════════════════════════════════════
    -- SEMANTIC CONVENTIONS: STORAGE
    -- ═══════════════════════════════════════════════════════════════════
    storage_system              VARCHAR,    -- s3, gcs, azure_blob
    storage_bucket              VARCHAR,    -- Bucket name
    storage_object              VARCHAR,    -- Object key

    -- ═══════════════════════════════════════════════════════════════════
    -- SEMANTIC CONVENTIONS: MESSAGING
    -- ═══════════════════════════════════════════════════════════════════
    messaging_system            VARCHAR,    -- kafka, sqs, rabbitmq
    messaging_destination       VARCHAR,    -- Topic/queue name

    -- ═══════════════════════════════════════════════════════════════════
    -- USER-DEFINED DATA
    -- ═══════════════════════════════════════════════════════════════════
    tags                        VARCHAR,    -- User-defined tags (JSON array)
    metadata                    JSON,       -- User-defined key-value pairs
    input_preview               VARCHAR,    -- Truncated input for UI display
    output_preview              VARCHAR,    -- Truncated output for UI display

    -- ═══════════════════════════════════════════════════════════════════
    -- RAW MESSAGES, TOOL DEFINITIONS, TOOL NAMES
    -- Stored as JSON array; converted to SideML on query
    -- ═══════════════════════════════════════════════════════════════════
    messages                    JSON NOT NULL DEFAULT '[]',
    tool_definitions            JSON NOT NULL DEFAULT '[]',
    tool_names                  JSON NOT NULL DEFAULT '[]',

    -- ═══════════════════════════════════════════════════════════════════
    -- RAW SPAN (original OTLP span for reconstruction/debugging)
    -- Stored as JSON for direct querying; includes attributes and resource.attributes
    -- ═══════════════════════════════════════════════════════════════════
    raw_span                    JSON,

    -- Instrumentation scope (v2). Declared last, deliberately: the span writer is a positional
    -- Appender and a migration can only append, so fresh and upgraded databases must agree on the
    -- physical order - the invariant `migration_added_columns_are_declared_last` pins.
    scope_name                  VARCHAR,
    scope_version               VARCHAR,
    -- Producer-content digest for strict staged-payload confirmation. Appended so an upgraded table
    -- has the same physical order as a fresh one.
    content_digest              VARCHAR NOT NULL DEFAULT '',
    hold_until                 TIMESTAMP,
    logical_bytes              UBIGINT NOT NULL DEFAULT 0,
);

-- Indexes for spans (minimal - DuckDB columnar scans are efficient for low-cardinality filters)
CREATE INDEX IF NOT EXISTS idx_spans_project_trace ON otel_spans(project_id, trace_id);
CREATE INDEX IF NOT EXISTS idx_spans_project_ts ON otel_spans(project_id, timestamp_start DESC);
CREATE INDEX IF NOT EXISTS idx_spans_project_ingest ON otel_spans(project_id, ingested_at DESC);
CREATE INDEX IF NOT EXISTS idx_spans_detail ON otel_spans(project_id, trace_id, span_id);
CREATE INDEX IF NOT EXISTS idx_spans_project_session ON otel_spans(project_id, session_id);
CREATE INDEX IF NOT EXISTS idx_spans_project_span ON otel_spans(project_id, span_id);

-- ═══════════════════════════════════════════════════════════════════════════════
-- OTEL metrics table: Main table for all OpenTelemetry metric data points
-- No PRIMARY KEY for append-only time-series ingestion
-- One row per data point (flattened from OTLP metric structures)
-- ═══════════════════════════════════════════════════════════════════════════════
CREATE TABLE IF NOT EXISTS otel_metrics (
    -- ═══════════════════════════════════════════════════════════════════
    -- IDENTITY
    -- ═══════════════════════════════════════════════════════════════════
    project_id              VARCHAR,            -- Tenant isolation
    metric_name             VARCHAR NOT NULL,   -- Metric name (e.g., "http.server.duration")
    metric_description      VARCHAR,            -- Human-readable description
    metric_unit             VARCHAR,            -- Unit (e.g., "ms", "By", "1")

    -- ═══════════════════════════════════════════════════════════════════
    -- METRIC TYPE & AGGREGATION
    -- ═══════════════════════════════════════════════════════════════════
    metric_type             VARCHAR NOT NULL,   -- gauge, sum, histogram, exponential_histogram, summary
    aggregation_temporality VARCHAR,            -- unspecified, cumulative, delta
    is_monotonic            BOOLEAN,            -- For sum type only

    -- ═══════════════════════════════════════════════════════════════════
    -- TIMING
    -- ═══════════════════════════════════════════════════════════════════
    timestamp               TIMESTAMP NOT NULL, -- Data point timestamp (UTC)
    start_timestamp         TIMESTAMP,          -- For cumulative metrics

    -- ═══════════════════════════════════════════════════════════════════
    -- VALUE (for Gauge/Sum types - one populated based on value type)
    -- ═══════════════════════════════════════════════════════════════════
    value_int               BIGINT,             -- Integer value
    value_double            DOUBLE,             -- Double value

    -- ═══════════════════════════════════════════════════════════════════
    -- HISTOGRAM AGGREGATES (for histogram type)
    -- ═══════════════════════════════════════════════════════════════════
    histogram_count         UBIGINT,            -- Number of values in histogram
    histogram_sum           DOUBLE,             -- Sum of values
    histogram_min           DOUBLE,             -- Minimum value
    histogram_max           DOUBLE,             -- Maximum value
    histogram_bucket_counts JSON,               -- [count, count, ...] for each bucket
    histogram_explicit_bounds JSON,             -- [bound, bound, ...] bucket boundaries

    -- ═══════════════════════════════════════════════════════════════════
    -- EXPONENTIAL HISTOGRAM (for exponential_histogram type)
    -- ═══════════════════════════════════════════════════════════════════
    exp_histogram_scale     INTEGER,            -- Base 2 exponent scale
    exp_histogram_zero_count UBIGINT,           -- Count of zero values
    exp_histogram_zero_threshold DOUBLE,        -- Zero bucket threshold
    exp_histogram_positive  JSON,               -- {offset, bucket_counts[]}
    exp_histogram_negative  JSON,               -- {offset, bucket_counts[]}

    -- ═══════════════════════════════════════════════════════════════════
    -- SUMMARY (for summary type)
    -- ═══════════════════════════════════════════════════════════════════
    summary_count           UBIGINT,            -- Total count
    summary_sum             DOUBLE,             -- Sum of values
    summary_quantiles       JSON,               -- [{quantile, value}, ...]

    -- ═══════════════════════════════════════════════════════════════════
    -- EXEMPLAR (the *first* exemplar, indexed for trace correlation)
    -- The full set is in `exemplars` at the end of the table.
    -- ═══════════════════════════════════════════════════════════════════
    exemplar_trace_id       VARCHAR,            -- Linked trace ID (hex)
    exemplar_span_id        VARCHAR,            -- Linked span ID (hex)
    exemplar_value_int      BIGINT,             -- Exemplar integer value
    exemplar_value_double   DOUBLE,             -- Exemplar double value
    exemplar_timestamp      TIMESTAMP,          -- Exemplar timestamp
    exemplar_attributes     JSON,               -- Exemplar filtered attributes

    -- ═══════════════════════════════════════════════════════════════════
    -- CONTEXT (extracted from attributes)
    -- ═══════════════════════════════════════════════════════════════════
    session_id              VARCHAR,            -- Conversation/session grouping
    user_id                 VARCHAR,            -- End user identifier
    environment             VARCHAR,            -- dev/staging/prod

    -- ═══════════════════════════════════════════════════════════════════
    -- RESOURCE
    -- ═══════════════════════════════════════════════════════════════════
    service_name            VARCHAR,            -- service.name
    service_version         VARCHAR,            -- service.version
    service_namespace       VARCHAR,            -- service.namespace
    service_instance_id     VARCHAR,            -- service.instance.id

    -- ═══════════════════════════════════════════════════════════════════
    -- INSTRUMENTATION SCOPE
    -- ═══════════════════════════════════════════════════════════════════
    scope_name              VARCHAR,            -- Scope name
    scope_version           VARCHAR,            -- Scope version

    -- ═══════════════════════════════════════════════════════════════════
    -- ATTRIBUTES
    -- ═══════════════════════════════════════════════════════════════════
    attributes              JSON,               -- Data point attributes
    resource_attributes     JSON,               -- Resource attributes

    -- ═══════════════════════════════════════════════════════════════════
    -- FLAGS & RAW
    -- ═══════════════════════════════════════════════════════════════════
    flags                   INTEGER,            -- OTLP data point flags
    raw_metric              JSON,               -- Raw metric JSON for debugging

    -- ═══════════════════════════════════════════════════════════════════
    -- IDENTITY (last, deliberately)
    -- ═══════════════════════════════════════════════════════════════════
    -- Series + instant identity; see domain::metrics::identity.
    --
    -- *Last* because the writer is DuckDB's `Appender`, which is positional, and `ALTER TABLE ADD COLUMN`
    -- appends. Declared where it reads best - beside `project_id` - a fresh database had it second while
    -- an upgraded one had it last, so the appender shifted every value by one column on exactly the
    -- databases a migration exists to serve. Fresh and upgraded have to agree on the physical order, and
    -- the only order a migration can produce is "at the end".
    datapoint_id            VARCHAR NOT NULL,
    -- Instrumentation scope attributes and the schema URLs, which OTel counts as part of what names a
    -- stream. Appended, like `datapoint_id`, for the reason above.
    scope_attributes        JSON,
    scope_schema_url        VARCHAR,
    resource_schema_url     VARCHAR,
    -- Every exemplar, not just the first. A histogram carries one per bucket, so the six flat
    -- `exemplar_*` columns above - which the trace-correlation index is built on - held one trace link
    -- out of however many the exporter sent. Appended, for the positional-Appender reason above.
    exemplars               JSON,
    -- Server receipt time, and the version that decides which re-delivery of a datapoint wins.
    -- ClickHouse sorts its replacing engine by it; this side compares it before replacing, so both
    -- backends answer that question the same way. Appended, for the positional-Appender reason above.
    --
    -- **Nullable, unlike the spans column**, and not by preference: DuckDB refuses
    -- `ALTER COLUMN ... SET NOT NULL` on a TIMESTAMP added in the same transaction ("Cannot create index
    -- with outstanding updates" - it succeeds in autocommit and fails inside one, the same class as the
    -- documented `UPDATE`-then-`SET NOT NULL` refusal, and `datapoint_id` escapes it only by being a
    -- VARCHAR). The migration therefore cannot produce a NOT NULL column, so the fresh schema must not
    -- claim one - `a_v1_database_upgrades_to_the_same_column_order_as_a_fresh_one` is what makes that
    -- agreement a requirement rather than a preference. Nothing exercises the nullability: the writer
    -- always supplies a value.
    ingested_at             TIMESTAMP DEFAULT (now())
    ,
    -- Producer-content digest for strict staged-payload confirmation. Appended after the v3
    -- version column because DuckDB's metric writer is positional.
    content_digest          VARCHAR NOT NULL DEFAULT ''
    ,
    hold_until             TIMESTAMP,
    logical_bytes          UBIGINT NOT NULL DEFAULT 0
);

-- Indexes for metrics
CREATE INDEX IF NOT EXISTS idx_metrics_project_ts ON otel_metrics(project_id, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_metrics_project_name ON otel_metrics(project_id, metric_name);
CREATE INDEX IF NOT EXISTS idx_metrics_project_name_ts ON otel_metrics(project_id, metric_name, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_metrics_exemplar_trace ON otel_metrics(project_id, exemplar_trace_id);
CREATE INDEX IF NOT EXISTS idx_metrics_session ON otel_metrics(project_id, session_id);

-- OTLP logs. The semantic digest plus per-export ordinal is the record identity; `ingested_at`
-- versions a correction consistently with the other two analytical signals.
CREATE TABLE IF NOT EXISTS otel_logs (
    project_id                VARCHAR NOT NULL,
    log_digest               VARCHAR NOT NULL,
    ordinal                  UINTEGER NOT NULL,
    timestamp                TIMESTAMP NOT NULL,
    time                     TIMESTAMP,
    observed_time            TIMESTAMP,
    severity_number          INTEGER NOT NULL,
    severity_text            VARCHAR,
    body                     JSON,
    body_text                VARCHAR,
    attributes               JSON,
    dropped_attributes_count UINTEGER NOT NULL,
    flags                     UINTEGER NOT NULL,
    trace_id                  VARCHAR,
    span_id                   VARCHAR,
    event_name                VARCHAR,
    session_id                VARCHAR,
    user_id                   VARCHAR,
    environment               VARCHAR,
    service_name              VARCHAR,
    service_version           VARCHAR,
    service_namespace         VARCHAR,
    service_instance_id       VARCHAR,
    resource_attributes       JSON,
    scope_name                VARCHAR,
    scope_version             VARCHAR,
    scope_attributes          JSON,
    scope_schema_url          VARCHAR,
    resource_schema_url       VARCHAR,
    raw_log                   JSON,
    ingested_at               TIMESTAMP NOT NULL,
    hold_until                TIMESTAMP,
    logical_bytes             UBIGINT NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_logs_identity
    ON otel_logs(project_id, log_digest, ordinal);
CREATE INDEX IF NOT EXISTS idx_logs_project_time
    ON otel_logs(project_id, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_logs_trace_span
    ON otel_logs(project_id, trace_id, span_id);
CREATE INDEX IF NOT EXISTS idx_logs_service
    ON otel_logs(project_id, service_name);

-- Current-version search terms. Corrections replace these rows in the same transaction as
-- appending the new source revision. An empty term is a field marker, not a searchable token.
CREATE TABLE IF NOT EXISTS span_terms (
    project_id VARCHAR NOT NULL,
    trace_id VARCHAR NOT NULL,
    span_id VARCHAR NOT NULL,
    field VARCHAR NOT NULL,
    term VARCHAR NOT NULL,
    truncated BOOLEAN NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_span_terms_term ON span_terms(term);

CREATE TABLE IF NOT EXISTS log_terms (
    project_id VARCHAR NOT NULL,
    log_digest VARCHAR NOT NULL,
    ordinal UINTEGER NOT NULL,
    field VARCHAR NOT NULL,
    term VARCHAR NOT NULL,
    truncated BOOLEAN NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_log_terms_term ON log_terms(term);

"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn test_schema_version_is_positive() {
        assert!(SCHEMA_VERSION > 0);
    }

    #[test]
    #[allow(clippy::const_is_empty)]
    fn test_schema_is_not_empty() {
        assert!(!SCHEMA.is_empty());
    }

    #[test]
    fn test_schema_contains_required_tables() {
        let required_tables = [
            "schema_version",
            "otel_spans",
            "otel_metrics",
            "otel_logs",
            "span_terms",
            "log_terms",
        ];

        for table in required_tables {
            assert!(
                SCHEMA.contains(&format!("CREATE TABLE IF NOT EXISTS {}", table)),
                "Schema missing table: {}",
                table
            );
        }

        assert!(
            !SCHEMA.contains("otel_spans_v"),
            "Schema should not contain removed otel_spans_v view"
        );
    }
}
