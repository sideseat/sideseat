//! DuckDB schema definitions.
//!
//! Long text columns are declared `USING COMPRESSION zstd`, which DuckDB honours in the storage format every
//! file is created in (`DUCKDB_STORAGE_VERSION` in the adapter).
//!
//! Append-only storage with no PRIMARY KEY constraints. A span identity keeps every revision it was delivered
//! as; `superseded_at` marks every one but the winner, so a read takes winners with a condition on the row
//! (`sideseat_query_sql::winners`).

/// Current schema version
pub const SCHEMA_VERSION: i32 = 2;

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
    exception_stacktrace VARCHAR USING COMPRESSION zstd,           -- Exception stacktrace

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
    gen_ai_usage_details            JSON USING COMPRESSION zstd,   -- Overflow: provider-specific usage

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
    metadata                    JSON USING COMPRESSION zstd,       -- User-defined key-value pairs
    input_preview               VARCHAR,    -- Truncated input for UI display
    output_preview              VARCHAR,    -- Truncated output for UI display

    -- ═══════════════════════════════════════════════════════════════════
    -- RAW MESSAGES, TOOL DEFINITIONS, TOOL NAMES
    -- Stored as JSON array; converted to SideML on query
    -- ═══════════════════════════════════════════════════════════════════
    messages                    JSON NOT NULL DEFAULT '[]' USING COMPRESSION zstd,
    tool_definitions            JSON NOT NULL DEFAULT '[]' USING COMPRESSION zstd,
    tool_names                  JSON NOT NULL DEFAULT '[]' USING COMPRESSION zstd,

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
    -- The stored raw record this row was derived from (otel_raw.raw_id).
    raw_id                     VARCHAR,
    -- How many events and links the span carries. The events and links themselves live in the raw record.
    event_count                UINTEGER NOT NULL DEFAULT 0,
    link_count                 UINTEGER NOT NULL DEFAULT 0,
    -- The ingest instant of the revision that follows this one, NULL while this is the identity's winner: what
    -- makes a winner a condition on the row rather than a window over the table (sideseat_query_sql::winners).
    superseded_at              TIMESTAMP,
    -- The search fields this revision indexed and those whose terms were cut short, one bit per field
    -- (sideseat_query_sql::search::duckdb_field_bit): a search reads a record's field state here, beside its
    -- terms. No bit set means not indexed yet, which the backfill picks up.
    search_fields              UTINYINT NOT NULL DEFAULT 0,
    search_truncated           UTINYINT NOT NULL DEFAULT 0,
    -- The conversation thread a request span belongs to, where a producer exports each request as what it added
    -- (sideseat_domain::rules::request_threads); NULL on every other span, which is almost all of them. Derived at
    -- ingest from the span's attributes, because a read holds a span's messages and not its attributes - a cache a
    -- re-parse rebuilds, like every extracted column. NULL rather than '' for the index's sake: DuckDB's ART holds
    -- no NULL keys, so it costs only the spans that have a thread, where an '' on every span put every row id in
    -- it.
    request_thread             VARCHAR,
    -- The declared read-time facts the span answered at ingest, one bit each
    -- (sideseat_domain::rules::span_marks); 0 where no mark holds, which is almost every span. Derived at ingest
    -- for the same reason as the thread key: a read holds a span's messages and not its attributes, and storing
    -- the answers bounds what that costs where storing the attributes again would not.
    span_marks                 USMALLINT NOT NULL DEFAULT 0,
    -- The key a detached request frame names this span by, where a carrier frames requests
    -- (CarrierRule::frames_requests); '' on every other span, which is almost all of them. Derived at ingest for
    -- the thread key's reason, and read only on the row a span view already holds, so no index.
    request_frame              VARCHAR NOT NULL DEFAULT '',
    -- The revision's exact search terms, one column per field spans search, as ClickHouse keeps them: each term
    -- between single spaces (search::duckdb_term_column), NULL for a field with none. On the row rather than in a
    -- term table, which repeated the span's identity and instant on each of about 32 terms: measured at thirty
    -- passes of the pinned corpus, 237 B a span there against 38 here. Each revision carries its own, and a search
    -- reads winners, so a correction writes no delete and a deleted row takes its terms with it. The prompt's and
    -- completion's terms are zstd, which their length needs - the prompt's are left uncompressed by the default,
    -- 162 B a span against 13 - and the short fields keep the default, which a search scans faster.
    search_prompt              VARCHAR USING COMPRESSION zstd,
    search_completion          VARCHAR USING COMPRESSION zstd,
    search_tool_name           VARCHAR,
    search_tool_args           VARCHAR,
    search_error               VARCHAR,
    search_span_name           VARCHAR
);

-- Indexes exist only where a read provably uses them. DuckDB reads through an ART index only for a scan whose
-- one filter is an equality or IN list on the column of a single-column index; it never scans through a compound
-- index, and a second predicate in the same scan reads the whole table. The six compound indexes this table had
-- were therefore never read - measured on a million spans, every keyed read scanned every row with them and
-- without - while they took a quarter of the file and every insert maintained them. What remains is one index per
-- key a read looks rows up by, each read written so the key is its scan's only filter (`sideseat_query_sql::keyed`):
-- `span_id` for the identity lookups an ingest makes, `trace_id` for reading a trace and for the tool spans a
-- composed request's calls sit in, and `request_thread` for the earlier requests of one thread. Deliberately no
-- index on `gen_ai_tool_call_id`: a composed request's calls are read through the trace index and narrowed by
-- their ids, so an index maintained on every span for the few that carry one buys nothing.
CREATE INDEX IF NOT EXISTS idx_spans_span ON otel_spans(span_id);
CREATE INDEX IF NOT EXISTS idx_spans_trace ON otel_spans(trace_id);
CREATE INDEX IF NOT EXISTS idx_spans_request_thread ON otel_spans(request_thread);

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
    -- EXEMPLARS: every one, each once, in the order sent (repositories::metric_exemplars). A histogram
    -- carries one per bucket. The first is the one the trace-correlation filter matches; the API's first
    -- exemplar fields and its array are rebuilt from this. NULL for a point with none. The ids stay hex:
    -- compressed, 32 hex characters cost 17.92 B where 16 bytes as a BLOB cost 18.56, measured on 409,710
    -- random ids, and text keeps an id of any length.
    -- ═══════════════════════════════════════════════════════════════════
    exemplars               STRUCT(trace_id VARCHAR, span_id VARCHAR, time_unix_nano UBIGINT,
                                   value_int BIGINT, value_double DOUBLE, attributes VARCHAR)[],

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
    attributes              JSON USING COMPRESSION zstd,               -- Data point attributes
    resource_attributes     JSON USING COMPRESSION zstd,               -- Resource attributes

    -- ═══════════════════════════════════════════════════════════════════
    -- FLAGS & RAW
    -- ═══════════════════════════════════════════════════════════════════
    flags                   INTEGER,            -- OTLP data point flags
    raw_metric              JSON USING COMPRESSION zstd,               -- Raw metric JSON for debugging

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
    scope_attributes        JSON USING COMPRESSION zstd,
    scope_schema_url        VARCHAR,
    resource_schema_url     VARCHAR,
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

-- No index. A datapoint's instant is part of its identity, so the lookups a write and its confirmation make are
-- bounded by the datapoints' own `timestamp`, which the row groups' zone maps answer; an index on `datapoint_id`
-- measured 85 bytes per point, more than half the per-point target. See the span indexes for why no compound
-- index is kept.

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
    body                     JSON USING COMPRESSION zstd,
    body_text                VARCHAR USING COMPRESSION zstd,
    attributes               JSON USING COMPRESSION zstd,
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
    resource_attributes       JSON USING COMPRESSION zstd,
    scope_name                VARCHAR,
    scope_version             VARCHAR,
    scope_attributes          JSON USING COMPRESSION zstd,
    scope_schema_url          VARCHAR,
    resource_schema_url       VARCHAR,
    raw_log                   JSON USING COMPRESSION zstd,
    ingested_at               TIMESTAMP NOT NULL,
    hold_until                TIMESTAMP,
    logical_bytes             UBIGINT NOT NULL DEFAULT 0,
    -- Raw messages a declared log event carries, joined to the span it names at read time. Not identity:
    -- `log_digest` covers the record, not this. NULL for a record that carries none, and for one whose raw_log
    -- rebuilds them (domain::log_messages), as nearly every record's does; stored where it does not, or where they
    -- hold a file reference a release must see. `messages_bytes` says which records carry any.
    messages                  VARCHAR USING COMPRESSION zstd,
    -- The search fields this record indexed and those it truncated, as on otel_spans.
    search_fields             UTINYINT NOT NULL DEFAULT 0,
    search_truncated          UTINYINT NOT NULL DEFAULT 0,
    -- The key the frame this record carries states, in its project's and trace's form (RequestFrames::stored_key):
    -- sixteen bytes, where its messages were read from a carrier that frames requests
    -- (CarrierRule::frames_requests); NULL on every other record, which a run of costs nothing. Derived at ingest
    -- like `messages`. A framed request's view reads by it.
    frame_key                 UHUGEINT,
    -- The references this record's views show as a note, as a JSON array; NULL on every record without one. With
    -- raw_log, what an unstored body, body_text and attributes are rebuilt from (domain::log_views): all three are
    -- NULL wherever the raw record rebuilds them, as nearly every record's does.
    noted_references          VARCHAR,
    -- The record's exact search terms, one column per field logs search, as ClickHouse keeps them: each term between
    -- single spaces (search::duckdb_term_column), NULL for a field with none. On the row rather than in a term
    -- table, which repeated the record's identity on every term, about fifty times a record. The body's terms are
    -- zstd, which their text needs; the attributes' keep the default, as small on the corpus and three times faster
    -- for a search to scan.
    search_body               VARCHAR USING COMPRESSION zstd,
    search_event_name         VARCHAR,
    search_severity           VARCHAR,
    search_attributes         VARCHAR,
    -- The length in bytes of the record's messages as their JSON array, stored or not; zero where it carries none.
    -- What a read asks for whether a record carries messages, and what the frame budget sums.
    messages_bytes            UINTEGER NOT NULL DEFAULT 0
);
-- No index on the identity. One row per identity is the write's: it replaces the identity's stored row with the
-- batch's winner, the highest `ingested_at` as ClickHouse's ReplacingMergeTree keeps, inside one transaction under
-- the store's single writer (`log::insert_batch`) - as a metric point's is. A unique index enforcing it again cost
-- about 98 bytes a record. The lookups a write and its confirmation make are bounded by the records' own
-- `timestamp` - part of the digest - which the zone maps answer. See the span indexes for why no other compound
-- index is kept.
-- The frames read, keyed by the one column it asks about: a trace's frames sit in whatever row groups their records
-- landed in, which statistics cannot skip once frames are spread over most of them. A record that frames nothing
-- stores NULL, which the index holds no entry for, so a store without frames pays nothing for it.
CREATE INDEX IF NOT EXISTS idx_logs_frame_key ON otel_logs(frame_key);

-- ═══════════════════════════════════════════════════════════════════════════════
-- Raw telemetry: every received export as an SSR1 record, the authority the other tables are derived from.
-- Append-only by (project_id, raw_id, version); readers take the latest version, which is the repair or the
-- rewrite after a deletion when there is one. The lifecycle is server/specs/RawRecordOwnership.tla.
-- ═══════════════════════════════════════════════════════════════════════════════
CREATE TABLE IF NOT EXISTS otel_raw (
    project_id   VARCHAR NOT NULL,
    raw_id       VARCHAR NOT NULL,
    signal       VARCHAR NOT NULL,
    received_at  TIMESTAMP NOT NULL,
    origin       VARCHAR NOT NULL,
    -- `(epoch << 32) | n`: the arbiter epoch the version was keyed under, then its place within it
    -- (`sideseat_ports::types::raw_version`). Named for the key, not the receipt instant versions once were.
    version_key  BIGINT NOT NULL,
    signal_until TIMESTAMP NOT NULL,
    hold_until   TIMESTAMP,
    record       BLOB NOT NULL USING COMPRESSION zstd
);
-- Every read of a record is by id: the repair, the confirmation and the insert that skips one already stored.
CREATE INDEX IF NOT EXISTS idx_raw_id ON otel_raw(raw_id);

-- Which records hold spans of which trace: how a deletion finds a record no row names any more.
CREATE TABLE IF NOT EXISTS otel_raw_traces (
    project_id   VARCHAR NOT NULL,
    trace_id     VARCHAR NOT NULL,
    raw_id       VARCHAR NOT NULL,
    signal_until TIMESTAMP NOT NULL,
    hold_until   TIMESTAMP
);
-- An insert asks whether a record's trace rows are already there, by record.
CREATE INDEX IF NOT EXISTS idx_raw_traces_raw ON otel_raw_traces(raw_id);

-- Records whose rows were deleted, expired or repaired, waiting for the reconciler. Each entry has a token so a
-- reconciler removes exactly the entries it read.
CREATE TABLE IF NOT EXISTS otel_raw_pending (
    project_id   VARCHAR NOT NULL,
    raw_id       VARCHAR NOT NULL,
    token        VARCHAR NOT NULL,
    enqueued_at  TIMESTAMP NOT NULL
);

-- The media references of the log rows that hold any, one row per log and reference: what a file release reads
-- to see which media a trace's logs still name, since otel_logs has no trace lookup. Written and deleted in the
-- transactions that write and delete the log rows, so it never outlives one. Rows exist only for media-bearing
-- logs, so the table costs nothing for a store without any, and a scan of it is bounded by them.
CREATE TABLE IF NOT EXISTS log_media_refs (
    project_id VARCHAR NOT NULL,
    trace_id VARCHAR NOT NULL,
    span_id VARCHAR,
    log_digest VARCHAR NOT NULL,
    ordinal UINTEGER NOT NULL,
    uri VARCHAR NOT NULL
);
-- A redelivery replaces its own identity's reference rows for the references it writes again, found by digest: the
-- write reads the rows of what it replaces, never the table. Indexing only rows that exist - media logs - costs
-- next to nothing.
CREATE INDEX IF NOT EXISTS idx_log_media_refs_digest ON log_media_refs(log_digest);

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
            "log_media_refs",
            "otel_raw",
            "otel_raw_pending",
            "otel_raw_traces",
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
