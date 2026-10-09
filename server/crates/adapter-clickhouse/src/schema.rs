//! ClickHouse schema definitions.
//!
//! Supports both single-node and distributed cluster deployments:
//! - Single-node: Uses ReplacingMergeTree for deduplication
//! - Distributed: Uses ReplicatedReplacingMergeTree with Distributed routing
//!
//! Optimized for high-throughput SaaS workloads:
//! - Sharding by project_id for tenant isolation
//! - Efficient ORDER BY for time-range queries
//! - Bloom filter indices for ID lookups
//! - TTL for automatic data expiration
//! - Projections for common aggregations

use sideseat_core::config::ClickhouseConfig;

/// Current schema version
pub const SCHEMA_VERSION: i32 = 2;

pub const TENANT_PROJECT_SETTING: &str = "SQL_sideseat_project_id";
pub const TENANT_MAINTENANCE_SETTING: &str = "SQL_sideseat_maintenance";

const TENANT_POLICY_EXPRESSION: &str = "project_id = \
getSettingOrDefault('SQL_sideseat_project_id', '') OR \
getSettingOrDefault('SQL_sideseat_maintenance', 0) = 1";

mod raw;
pub use raw::raw_tables;

/// Validate and return a cluster name safe for SQL interpolation.
///
/// ClickHouse cluster names may contain alphanumeric characters, underscores,
/// hyphens, and dots. This prevents SQL injection via the config file.
fn safe_cluster_name(config: &ClickhouseConfig) -> &str {
    let name = config.cluster.as_deref().unwrap_or("default");
    assert!(
        !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.'),
        "Invalid ClickHouse cluster name: {name:?}. Only alphanumeric, underscore, hyphen, and dot are allowed."
    );
    name
}

/// Quote the configured database as one ClickHouse identifier.
///
/// In particular, this must not be interpolated as a bare token into access-control DDL. A row
/// policy issued `ON CLUSTER` without an explicitly qualified table is created against `default`
/// on the remote nodes, even when the initiating client selected another database.
pub(crate) fn database_identifier(config: &ClickhouseConfig) -> String {
    use clickhouse::sql::{Bind, Identifier};

    let mut quoted = String::new();
    Identifier(&config.database)
        .write(&mut quoted)
        .expect("writing to String cannot fail");
    quoted
}

/// Where the cross-partition consistency check keeps its findings and its place.
///
/// **Why a durable record and not a log line.** The check reports identities whose revisions sit in more than
/// one partition. The state that produces the finding can
/// disappear while the damage persists: when a correction moves *backward* across a month, the newer revision
/// expires first and the obsolete one is left alone in its partition - a current-state query then sees one row
/// per identity and reports clean, having permanently served the wrong revision. Only a record written at the
/// time survives that.
///
/// **Why in ClickHouse rather than the transactional store.** The finding is a statement about rows in this
/// store, so it belongs beside them - and step 0 deliberately keeps this adapter free of a transactional
/// dependency that the crate split would immediately have to dismantle. The consequence is stated rather than
/// hidden: restoring ClickHouse to a point before the check ran loses the record with the evidence it
/// describes.
///
/// `checked_through` is the watermark, stored as a row of the same table keyed by an empty identity: one table
/// rather than two, because the watermark is only meaningful together with what was found under it. A
/// `ReplacingMergeTree` on `(project_id, trace_id, span_id)` means re-detecting the same identity updates its
/// record rather than accumulating a row per pass.
pub fn consistency_tables(config: &ClickhouseConfig) -> Vec<String> {
    let (engine, on_cluster, local) = if config.distributed {
        (
            format!(
                "ReplicatedReplacingMergeTree('/clickhouse/tables/{{shard}}/{db}/span_partition_anomalies_local', '{{replica}}', detected_at)",
                db = config.database
            ),
            format!(" ON CLUSTER {}", safe_cluster_name(config)),
            "_local",
        )
    } else {
        (
            "ReplacingMergeTree(detected_at)".to_string(),
            String::new(),
            "",
        )
    };

    let mut statements = vec![format!(
        r#"
CREATE TABLE IF NOT EXISTS span_partition_anomalies{local}{on_cluster} (
    project_id String,
    trace_id String,
    span_id String,
    partitions Array(String),
    revisions UInt32,
    detected_at DateTime64(6, 'UTC') DEFAULT now64(6),
    checked_through DateTime64(6, 'UTC') DEFAULT toDateTime64(0, 6, 'UTC')
) ENGINE = {engine}
ORDER BY (project_id, trace_id, span_id)
"#
    )];

    // **A `Distributed` front end, in distributed mode.** Without one this table is per-shard: a pass connected
    // to shard A records an anomaly there and a read that reaches shard B returns nothing, so a report described
    // as deployment-wide depends on which shard answered - and the watermark is worse, because each shard would
    // keep its own and re-scan windows the others had already covered.
    //
    // Sharded by identity rather than by `project_id`, unlike the span and metric tables: this table's rows are
    // written by whichever instance ran the pass, so keying on the project would put every anomaly of a busy
    // project on one shard while the pass that found them ran anywhere.
    if config.distributed {
        statements.push(format!(
            r#"
CREATE TABLE IF NOT EXISTS span_partition_anomalies{on_cluster} AS span_partition_anomalies_local
ENGINE = Distributed({cluster}, {db}, span_partition_anomalies_local,
                     sipHash64(project_id, trace_id, span_id))
"#,
            cluster = safe_cluster_name(config),
            db = config.database
        ));
    }

    statements
}

/// Generate schema version table
pub fn schema_version_table(config: &ClickhouseConfig) -> String {
    let engine = if config.distributed {
        format!(
            "ReplicatedReplacingMergeTree('/clickhouse/tables/{{shard}}/{db}/schema_version', '{{replica}}')",
            db = config.database
        )
    } else {
        "ReplacingMergeTree()".to_string()
    };

    let on_cluster = if config.distributed {
        format!(" ON CLUSTER {}", safe_cluster_name(config))
    } else {
        String::new()
    };

    format!(
        r#"
CREATE TABLE IF NOT EXISTS schema_version{on_cluster} (
    id UInt8,
    version Int32,
    applied_at Int64,
    description Nullable(String)
) ENGINE = {engine}
ORDER BY id
"#,
        on_cluster = on_cluster,
        engine = engine
    )
}

/// Generate OTEL spans local table (for distributed mode)
fn otel_spans_local_table(config: &ClickhouseConfig) -> String {
    let cluster = safe_cluster_name(config);
    let ttl_clause = "TTL greatest(timestamp_start + INTERVAL 90 DAY, coalesce(hold_until, toDateTime64(0, 6, 'UTC'))) DELETE";

    format!(
        r#"
CREATE TABLE IF NOT EXISTS otel_spans_local ON CLUSTER {cluster} (
    -- IDENTITY
    project_id              LowCardinality(String),
    trace_id                String,
    span_id                 String,
    parent_span_id          Nullable(String),
    trace_state             Nullable(String),

    -- CONTEXT
    session_id              Nullable(String),
    user_id                 Nullable(String),
    environment             LowCardinality(Nullable(String)),

    -- SPAN METADATA
    span_name               Nullable(String),
    span_kind               LowCardinality(Nullable(String)),
    status_code             LowCardinality(Nullable(String)),
    status_message          Nullable(String),
    exception_type          Nullable(String),
    exception_message       Nullable(String),
    exception_stacktrace    Nullable(String),

    -- CLASSIFICATION
    span_category           LowCardinality(Nullable(String)),
    observation_type        LowCardinality(Nullable(String)),
    framework               LowCardinality(Nullable(String)),

    -- TIMING
    timestamp_start         DateTime64(6, 'UTC'),
    timestamp_end           Nullable(DateTime64(6, 'UTC')),
    duration_ms             Nullable(Int64),
    ingested_at             DateTime64(6, 'UTC') DEFAULT now64(6),

    -- GEN AI: PROVIDER & MODEL
    gen_ai_system               LowCardinality(Nullable(String)),
    gen_ai_operation_name       LowCardinality(Nullable(String)),
    gen_ai_request_model        LowCardinality(Nullable(String)),
    gen_ai_response_model       LowCardinality(Nullable(String)),
    gen_ai_response_id          Nullable(String),

    -- GEN AI: REQUEST PARAMETERS
    gen_ai_temperature          Nullable(Float64),
    gen_ai_top_p                Nullable(Float64),
    gen_ai_top_k                Nullable(Int64),
    gen_ai_max_tokens           Nullable(Int64),
    gen_ai_frequency_penalty    Nullable(Float64),
    gen_ai_presence_penalty     Nullable(Float64),
    gen_ai_stop_sequences       Nullable(String),

    -- GEN AI: RESPONSE METADATA
    gen_ai_finish_reasons       Nullable(String),

    -- GEN AI: AGENT & TOOL
    gen_ai_agent_id             Nullable(String),
    gen_ai_agent_name           Nullable(String),
    gen_ai_tool_name            Nullable(String),
    gen_ai_tool_call_id         Nullable(String),

    -- GEN AI: PERFORMANCE METRICS
    gen_ai_server_ttft_ms       Nullable(Int64),
    gen_ai_server_request_duration_ms Nullable(Int64),

    -- GEN AI: TOKEN USAGE
    gen_ai_usage_input_tokens       Int64 DEFAULT 0,
    gen_ai_usage_output_tokens      Int64 DEFAULT 0,
    gen_ai_usage_total_tokens       Int64 DEFAULT 0,
    gen_ai_usage_cache_read_tokens  Int64 DEFAULT 0,
    gen_ai_usage_cache_write_tokens Int64 DEFAULT 0,
    gen_ai_usage_reasoning_tokens   Int64 DEFAULT 0,
    gen_ai_usage_details            Nullable(String),

    -- GEN AI: COSTS (Decimal for precision)
    gen_ai_cost_input           Decimal64(6) DEFAULT 0,
    gen_ai_cost_output          Decimal64(6) DEFAULT 0,
    gen_ai_cost_cache_read      Decimal64(6) DEFAULT 0,
    gen_ai_cost_cache_write     Decimal64(6) DEFAULT 0,
    gen_ai_cost_reasoning       Decimal64(6) DEFAULT 0,
    gen_ai_cost_total           Decimal64(6) DEFAULT 0,

    -- HTTP
    http_method                 LowCardinality(Nullable(String)),
    http_url                    Nullable(String),
    http_status_code            Nullable(Int32),

    -- DATABASE
    db_system                   LowCardinality(Nullable(String)),
    db_name                     Nullable(String),
    db_operation                LowCardinality(Nullable(String)),
    db_statement                Nullable(String),

    -- STORAGE
    storage_system              LowCardinality(Nullable(String)),
    storage_bucket              Nullable(String),
    storage_object              Nullable(String),

    -- MESSAGING
    messaging_system            LowCardinality(Nullable(String)),
    messaging_destination       Nullable(String),

    -- USER-DEFINED DATA
    tags                        Nullable(String),
    metadata                    Nullable(String),
    input_preview               Nullable(String),
    output_preview              Nullable(String),

    -- RAW MESSAGES, TOOL DEFINITIONS
    messages                    String DEFAULT '[]',
    tool_definitions            String DEFAULT '[]',
    tool_names                  String DEFAULT '[]',

    -- RAW SPAN (compressed)

    -- Instrumentation scope: the library that produced the span, versioned
    scope_name                  Nullable(String) CODEC(ZSTD(1)),
    scope_version               Nullable(String) CODEC(ZSTD(1)),
    content_digest              String DEFAULT '',
    hold_until                 Nullable(DateTime64(6, 'UTC')),
    logical_bytes              UInt64 DEFAULT 0,
    raw_id                     Nullable(String),
    event_count                UInt32 DEFAULT 0,
    link_count                 UInt32 DEFAULT 0,
    search_indexed             UInt8 DEFAULT 0,
    search_prompt              Array(String) DEFAULT [],
    search_prompt_truncated    UInt8 DEFAULT 0,
    search_completion          Array(String) DEFAULT [],
    search_completion_truncated UInt8 DEFAULT 0,
    search_tool_name           Array(String) DEFAULT [],
    search_tool_name_truncated UInt8 DEFAULT 0,
    search_tool_args           Array(String) DEFAULT [],
    search_tool_args_truncated UInt8 DEFAULT 0,
    search_error               Array(String) DEFAULT [],
    search_error_truncated     UInt8 DEFAULT 0,
    search_span_name           Array(String) DEFAULT [],
    search_span_name_truncated UInt8 DEFAULT 0,
    -- The conversation thread a request span belongs to, where a producer exports each request as what it added
    -- (sideseat_domain::rules::request_threads); '' on every other span, which is almost all of them. Not
    -- Nullable: a null map costs a byte on every span, and '' already means "no thread". LowCardinality would
    -- hold a dictionary per part for values that are nearly unique within a project.
    request_thread             String DEFAULT '' CODEC(ZSTD(1)),
    -- The declared read-time facts the span answered at ingest, one bit each
    -- (sideseat_domain::rules::span_marks); 0 where no mark holds, which is almost every span. A word rather
    -- than a column per mark, because the marks are one bounded set and a run of zeroes compresses to nothing.
    -- No index: a projection asks about a mark for a row it is already reading, never to find rows.
    span_marks                 UInt16 DEFAULT 0 CODEC(ZSTD(1)),

    -- INDICES for fast lookups
    INDEX idx_request_thread request_thread TYPE bloom_filter GRANULARITY 1,
    INDEX idx_trace_id trace_id TYPE bloom_filter GRANULARITY 1,
    INDEX idx_session_id session_id TYPE bloom_filter GRANULARITY 1,
    INDEX idx_span_id span_id TYPE bloom_filter GRANULARITY 1,
    INDEX idx_user_id user_id TYPE bloom_filter GRANULARITY 4,
    INDEX idx_gen_ai_system gen_ai_system TYPE bloom_filter GRANULARITY 4,
    INDEX idx_gen_ai_request_model gen_ai_request_model TYPE bloom_filter GRANULARITY 4,
    INDEX idx_observation_type observation_type TYPE set(0) GRANULARITY 4,
    INDEX idx_search_prompt search_prompt TYPE text(tokenizer = 'array') GRANULARITY 1,
    INDEX idx_search_completion search_completion TYPE text(tokenizer = 'array') GRANULARITY 1,
    INDEX idx_search_tool_name search_tool_name TYPE text(tokenizer = 'array') GRANULARITY 1,
    INDEX idx_search_tool_args search_tool_args TYPE text(tokenizer = 'array') GRANULARITY 1,
    INDEX idx_search_error search_error TYPE text(tokenizer = 'array') GRANULARITY 1,
    INDEX idx_search_span_name search_span_name TYPE text(tokenizer = 'array') GRANULARITY 1,
    -- `ingested_at` is neither the partition key nor in the sorting key, so "rows ingested since the last
    -- run" is otherwise a full scan - which is what would make the cross-partition consistency check
    -- (`consistency.rs`) too expensive to run continuously, and a check nobody runs reports nothing. A
    -- `minmax` index works here specifically because parts are roughly insertion-ordered, so a part's
    -- [min, max] range for this column is narrow and most parts prune.
    INDEX idx_ingested_at ingested_at TYPE minmax GRANULARITY 1
) ENGINE = ReplicatedReplacingMergeTree('/clickhouse/tables/{{shard}}/{db}/otel_spans', '{{replica}}', ingested_at)
PARTITION BY toYYYYMM(timestamp_start)
ORDER BY (project_id, trace_id, span_id)
{ttl_clause}
SETTINGS index_granularity = 8192, merge_with_ttl_timeout = 3600
"#,
        cluster = cluster,
        db = config.database,
        ttl_clause = ttl_clause
    )
}

/// Generate OTEL spans distributed table
fn otel_spans_distributed_table(config: &ClickhouseConfig) -> String {
    let cluster = safe_cluster_name(config);

    format!(
        r#"
CREATE TABLE IF NOT EXISTS otel_spans ON CLUSTER {cluster} AS otel_spans_local
ENGINE = Distributed('{cluster}', '{db}', 'otel_spans_local', sipHash64(project_id))
"#,
        cluster = cluster,
        db = config.database
    )
}

/// Generate OTEL spans table (single-node mode)
fn otel_spans_single_table() -> String {
    r#"
CREATE TABLE IF NOT EXISTS otel_spans (
    -- IDENTITY
    project_id              LowCardinality(String),
    trace_id                String,
    span_id                 String,
    parent_span_id          Nullable(String),
    trace_state             Nullable(String),

    -- CONTEXT
    session_id              Nullable(String),
    user_id                 Nullable(String),
    environment             LowCardinality(Nullable(String)),

    -- SPAN METADATA
    span_name               Nullable(String),
    span_kind               LowCardinality(Nullable(String)),
    status_code             LowCardinality(Nullable(String)),
    status_message          Nullable(String),
    exception_type          Nullable(String),
    exception_message       Nullable(String),
    exception_stacktrace    Nullable(String),

    -- CLASSIFICATION
    span_category           LowCardinality(Nullable(String)),
    observation_type        LowCardinality(Nullable(String)),
    framework               LowCardinality(Nullable(String)),

    -- TIMING
    timestamp_start         DateTime64(6, 'UTC'),
    timestamp_end           Nullable(DateTime64(6, 'UTC')),
    duration_ms             Nullable(Int64),
    ingested_at             DateTime64(6, 'UTC') DEFAULT now64(6),

    -- GEN AI: PROVIDER & MODEL
    gen_ai_system               LowCardinality(Nullable(String)),
    gen_ai_operation_name       LowCardinality(Nullable(String)),
    gen_ai_request_model        LowCardinality(Nullable(String)),
    gen_ai_response_model       LowCardinality(Nullable(String)),
    gen_ai_response_id          Nullable(String),

    -- GEN AI: REQUEST PARAMETERS
    gen_ai_temperature          Nullable(Float64),
    gen_ai_top_p                Nullable(Float64),
    gen_ai_top_k                Nullable(Int64),
    gen_ai_max_tokens           Nullable(Int64),
    gen_ai_frequency_penalty    Nullable(Float64),
    gen_ai_presence_penalty     Nullable(Float64),
    gen_ai_stop_sequences       Nullable(String),

    -- GEN AI: RESPONSE METADATA
    gen_ai_finish_reasons       Nullable(String),

    -- GEN AI: AGENT & TOOL
    gen_ai_agent_id             Nullable(String),
    gen_ai_agent_name           Nullable(String),
    gen_ai_tool_name            Nullable(String),
    gen_ai_tool_call_id         Nullable(String),

    -- GEN AI: PERFORMANCE METRICS
    gen_ai_server_ttft_ms       Nullable(Int64),
    gen_ai_server_request_duration_ms Nullable(Int64),

    -- GEN AI: TOKEN USAGE
    gen_ai_usage_input_tokens       Int64 DEFAULT 0,
    gen_ai_usage_output_tokens      Int64 DEFAULT 0,
    gen_ai_usage_total_tokens       Int64 DEFAULT 0,
    gen_ai_usage_cache_read_tokens  Int64 DEFAULT 0,
    gen_ai_usage_cache_write_tokens Int64 DEFAULT 0,
    gen_ai_usage_reasoning_tokens   Int64 DEFAULT 0,
    gen_ai_usage_details            Nullable(String),

    -- GEN AI: COSTS (Decimal for precision)
    gen_ai_cost_input           Decimal64(6) DEFAULT 0,
    gen_ai_cost_output          Decimal64(6) DEFAULT 0,
    gen_ai_cost_cache_read      Decimal64(6) DEFAULT 0,
    gen_ai_cost_cache_write     Decimal64(6) DEFAULT 0,
    gen_ai_cost_reasoning       Decimal64(6) DEFAULT 0,
    gen_ai_cost_total           Decimal64(6) DEFAULT 0,

    -- HTTP
    http_method                 LowCardinality(Nullable(String)),
    http_url                    Nullable(String),
    http_status_code            Nullable(Int32),

    -- DATABASE
    db_system                   LowCardinality(Nullable(String)),
    db_name                     Nullable(String),
    db_operation                LowCardinality(Nullable(String)),
    db_statement                Nullable(String),

    -- STORAGE
    storage_system              LowCardinality(Nullable(String)),
    storage_bucket              Nullable(String),
    storage_object              Nullable(String),

    -- MESSAGING
    messaging_system            LowCardinality(Nullable(String)),
    messaging_destination       Nullable(String),

    -- USER-DEFINED DATA
    tags                        Nullable(String),
    metadata                    Nullable(String),
    input_preview               Nullable(String),
    output_preview              Nullable(String),

    -- RAW MESSAGES, TOOL DEFINITIONS
    messages                    String DEFAULT '[]',
    tool_definitions            String DEFAULT '[]',
    tool_names                  String DEFAULT '[]',

    -- RAW SPAN (compressed)

    -- Instrumentation scope: the library that produced the span, versioned
    scope_name                  Nullable(String) CODEC(ZSTD(1)),
    scope_version               Nullable(String) CODEC(ZSTD(1)),
    content_digest              String DEFAULT '',
    hold_until                 Nullable(DateTime64(6, 'UTC')),
    logical_bytes              UInt64 DEFAULT 0,
    raw_id                     Nullable(String),
    event_count                UInt32 DEFAULT 0,
    link_count                 UInt32 DEFAULT 0,
    search_indexed             UInt8 DEFAULT 0,
    search_prompt              Array(String) DEFAULT [],
    search_prompt_truncated    UInt8 DEFAULT 0,
    search_completion          Array(String) DEFAULT [],
    search_completion_truncated UInt8 DEFAULT 0,
    search_tool_name           Array(String) DEFAULT [],
    search_tool_name_truncated UInt8 DEFAULT 0,
    search_tool_args           Array(String) DEFAULT [],
    search_tool_args_truncated UInt8 DEFAULT 0,
    search_error               Array(String) DEFAULT [],
    search_error_truncated     UInt8 DEFAULT 0,
    search_span_name           Array(String) DEFAULT [],
    search_span_name_truncated UInt8 DEFAULT 0,
    -- The conversation thread a request span belongs to, where a producer exports each request as what it added
    -- (sideseat_domain::rules::request_threads); '' on every other span, which is almost all of them. Not
    -- Nullable: a null map costs a byte on every span, and '' already means "no thread". LowCardinality would
    -- hold a dictionary per part for values that are nearly unique within a project.
    request_thread             String DEFAULT '' CODEC(ZSTD(1)),
    -- The declared read-time facts the span answered at ingest, one bit each
    -- (sideseat_domain::rules::span_marks); 0 where no mark holds, which is almost every span. A word rather
    -- than a column per mark, because the marks are one bounded set and a run of zeroes compresses to nothing.
    -- No index: a projection asks about a mark for a row it is already reading, never to find rows.
    span_marks                 UInt16 DEFAULT 0 CODEC(ZSTD(1)),

    -- INDICES for fast lookups
    INDEX idx_request_thread request_thread TYPE bloom_filter GRANULARITY 1,
    INDEX idx_trace_id trace_id TYPE bloom_filter GRANULARITY 1,
    INDEX idx_session_id session_id TYPE bloom_filter GRANULARITY 1,
    INDEX idx_span_id span_id TYPE bloom_filter GRANULARITY 1,
    INDEX idx_user_id user_id TYPE bloom_filter GRANULARITY 4,
    INDEX idx_gen_ai_system gen_ai_system TYPE bloom_filter GRANULARITY 4,
    INDEX idx_gen_ai_request_model gen_ai_request_model TYPE bloom_filter GRANULARITY 4,
    INDEX idx_observation_type observation_type TYPE set(0) GRANULARITY 4,
    INDEX idx_search_prompt search_prompt TYPE text(tokenizer = 'array') GRANULARITY 1,
    INDEX idx_search_completion search_completion TYPE text(tokenizer = 'array') GRANULARITY 1,
    INDEX idx_search_tool_name search_tool_name TYPE text(tokenizer = 'array') GRANULARITY 1,
    INDEX idx_search_tool_args search_tool_args TYPE text(tokenizer = 'array') GRANULARITY 1,
    INDEX idx_search_error search_error TYPE text(tokenizer = 'array') GRANULARITY 1,
    INDEX idx_search_span_name search_span_name TYPE text(tokenizer = 'array') GRANULARITY 1,
    -- `ingested_at` is neither the partition key nor in the sorting key, so "rows ingested since the last
    -- run" is otherwise a full scan - which is what would make the cross-partition consistency check
    -- (`consistency.rs`) too expensive to run continuously, and a check nobody runs reports nothing. A
    -- `minmax` index works here specifically because parts are roughly insertion-ordered, so a part's
    -- [min, max] range for this column is narrow and most parts prune.
    INDEX idx_ingested_at ingested_at TYPE minmax GRANULARITY 1
) ENGINE = ReplacingMergeTree(ingested_at)
PARTITION BY toYYYYMM(timestamp_start)
ORDER BY (project_id, trace_id, span_id)
TTL greatest(timestamp_start + INTERVAL 90 DAY, coalesce(hold_until, toDateTime64(0, 6, 'UTC'))) DELETE
SETTINGS index_granularity = 8192, merge_with_ttl_timeout = 3600
"#
    .to_string()
}

/// Generate OTEL metrics local table (for distributed mode)
fn otel_metrics_local_table(config: &ClickhouseConfig) -> String {
    let cluster = safe_cluster_name(config);

    format!(
        r#"
CREATE TABLE IF NOT EXISTS otel_metrics_local ON CLUSTER {cluster} (
    -- IDENTITY
    project_id              LowCardinality(String),
    datapoint_id            String,
    metric_name             LowCardinality(String),
    metric_description      Nullable(String),
    metric_unit             LowCardinality(Nullable(String)),

    -- METRIC TYPE
    metric_type             LowCardinality(String),
    aggregation_temporality LowCardinality(Nullable(String)),
    is_monotonic            Nullable(UInt8),

    -- TIMING
    timestamp               DateTime64(6, 'UTC'),
    start_timestamp         Nullable(DateTime64(6, 'UTC')),

    -- VALUE
    value_int               Nullable(Int64),
    value_double            Nullable(Float64),

    -- HISTOGRAM
    histogram_count         Nullable(UInt64),
    histogram_sum           Nullable(Float64),
    histogram_min           Nullable(Float64),
    histogram_max           Nullable(Float64),
    histogram_bucket_counts Nullable(String),
    histogram_explicit_bounds Nullable(String),

    -- EXPONENTIAL HISTOGRAM
    exp_histogram_scale     Nullable(Int32),
    exp_histogram_zero_count Nullable(UInt64),
    exp_histogram_zero_threshold Nullable(Float64),
    exp_histogram_positive  Nullable(String),
    exp_histogram_negative  Nullable(String),

    -- SUMMARY
    summary_count           Nullable(UInt64),
    summary_sum             Nullable(Float64),
    summary_quantiles       Nullable(String),

    -- EXEMPLAR (the first one; the full set is in `exemplars` below)
    exemplar_trace_id       Nullable(String),
    exemplar_span_id        Nullable(String),
    exemplar_value_int      Nullable(Int64),
    exemplar_value_double   Nullable(Float64),
    exemplar_timestamp      Nullable(DateTime64(6, 'UTC')),
    exemplar_attributes     Nullable(String),

    -- CONTEXT
    session_id              Nullable(String),
    user_id                 Nullable(String),
    environment             LowCardinality(Nullable(String)),

    -- RESOURCE
    service_name            LowCardinality(Nullable(String)),
    service_version         Nullable(String),
    service_namespace       Nullable(String),
    service_instance_id     Nullable(String),

    -- INSTRUMENTATION SCOPE
    scope_name              Nullable(String),
    scope_version           Nullable(String),

    -- ATTRIBUTES
    attributes              Nullable(String),
    resource_attributes     Nullable(String),

    -- FLAGS & RAW
    flags                   Nullable(Int32),
    raw_metric              Nullable(String) CODEC(ZSTD(3)),

    -- SCOPE AND SCHEMA
    scope_attributes        Nullable(String),
    scope_schema_url        Nullable(String),
    resource_schema_url     Nullable(String),

    -- Every exemplar, not only the first. A histogram carries one per bucket, so the flat exemplar_*
    -- columns above hold one trace link out of however many the exporter sent.
    exemplars               Nullable(String) CODEC(ZSTD(3)),

    -- The replacing engine's version. Without it the engine had no version argument at all, so which
    -- of two deliveries of one `datapoint_id` survived was insert-block order - while DuckDB deletes
    -- and re-inserts, making it commit-last-wins there. Two rules for one question, and a corrected
    -- datapoint could read differently per backend.
    ingested_at             DateTime64(6, 'UTC') DEFAULT now64(6),
    content_digest          String DEFAULT '',
    hold_until             Nullable(DateTime64(6, 'UTC')),
    logical_bytes          UInt64 DEFAULT 0,

    -- INDEXES
    INDEX idx_metric_name metric_name TYPE bloom_filter GRANULARITY 1,
    INDEX idx_session_id session_id TYPE bloom_filter GRANULARITY 1,
    -- The datapoint lookups a confirmation makes. The sorting key puts the metric name and time before the
    -- datapoint, so without this a lookup read every granule of its project; measured over a million points,
    -- 123 of 123 granules without it and 3 with it, for about a byte per point.
    INDEX idx_datapoint_id datapoint_id TYPE bloom_filter GRANULARITY 1
) ENGINE = ReplicatedReplacingMergeTree('/clickhouse/tables/{{shard}}/{db}/otel_metrics', '{{replica}}', ingested_at)
PARTITION BY toYYYYMM(timestamp)
ORDER BY (project_id, metric_name, toDate(timestamp), timestamp, datapoint_id)
TTL greatest(timestamp + INTERVAL 90 DAY, coalesce(hold_until, toDateTime64(0, 6, 'UTC'))) DELETE
SETTINGS index_granularity = 8192
"#,
        cluster = cluster,
        db = config.database
    )
}

/// Generate OTEL metrics distributed table
fn otel_metrics_distributed_table(config: &ClickhouseConfig) -> String {
    let cluster = safe_cluster_name(config);

    format!(
        r#"
CREATE TABLE IF NOT EXISTS otel_metrics ON CLUSTER {cluster} AS otel_metrics_local
ENGINE = Distributed('{cluster}', '{db}', 'otel_metrics_local', sipHash64(project_id))
"#,
        cluster = cluster,
        db = config.database
    )
}

/// Generate OTEL metrics table (single-node mode)
fn otel_metrics_single_table() -> String {
    r#"
CREATE TABLE IF NOT EXISTS otel_metrics (
    -- IDENTITY
    project_id              LowCardinality(String),
    datapoint_id            String,
    metric_name             LowCardinality(String),
    metric_description      Nullable(String),
    metric_unit             LowCardinality(Nullable(String)),

    -- METRIC TYPE
    metric_type             LowCardinality(String),
    aggregation_temporality LowCardinality(Nullable(String)),
    is_monotonic            Nullable(UInt8),

    -- TIMING
    timestamp               DateTime64(6, 'UTC'),
    start_timestamp         Nullable(DateTime64(6, 'UTC')),

    -- VALUE
    value_int               Nullable(Int64),
    value_double            Nullable(Float64),

    -- HISTOGRAM
    histogram_count         Nullable(UInt64),
    histogram_sum           Nullable(Float64),
    histogram_min           Nullable(Float64),
    histogram_max           Nullable(Float64),
    histogram_bucket_counts Nullable(String),
    histogram_explicit_bounds Nullable(String),

    -- EXPONENTIAL HISTOGRAM
    exp_histogram_scale     Nullable(Int32),
    exp_histogram_zero_count Nullable(UInt64),
    exp_histogram_zero_threshold Nullable(Float64),
    exp_histogram_positive  Nullable(String),
    exp_histogram_negative  Nullable(String),

    -- SUMMARY
    summary_count           Nullable(UInt64),
    summary_sum             Nullable(Float64),
    summary_quantiles       Nullable(String),

    -- EXEMPLAR (the first one; the full set is in `exemplars` below)
    exemplar_trace_id       Nullable(String),
    exemplar_span_id        Nullable(String),
    exemplar_value_int      Nullable(Int64),
    exemplar_value_double   Nullable(Float64),
    exemplar_timestamp      Nullable(DateTime64(6, 'UTC')),
    exemplar_attributes     Nullable(String),

    -- CONTEXT
    session_id              Nullable(String),
    user_id                 Nullable(String),
    environment             LowCardinality(Nullable(String)),

    -- RESOURCE
    service_name            LowCardinality(Nullable(String)),
    service_version         Nullable(String),
    service_namespace       Nullable(String),
    service_instance_id     Nullable(String),

    -- INSTRUMENTATION SCOPE
    scope_name              Nullable(String),
    scope_version           Nullable(String),

    -- ATTRIBUTES
    attributes              Nullable(String),
    resource_attributes     Nullable(String),

    -- FLAGS & RAW
    flags                   Nullable(Int32),
    raw_metric              Nullable(String) CODEC(ZSTD(3)),

    -- SCOPE AND SCHEMA
    scope_attributes        Nullable(String),
    scope_schema_url        Nullable(String),
    resource_schema_url     Nullable(String),

    -- Every exemplar, not only the first. A histogram carries one per bucket, so the flat exemplar_*
    -- columns above hold one trace link out of however many the exporter sent.
    exemplars               Nullable(String) CODEC(ZSTD(3)),

    -- The replacing engine's version - see the note on the local table.
    ingested_at             DateTime64(6, 'UTC') DEFAULT now64(6),
    content_digest          String DEFAULT '',
    hold_until             Nullable(DateTime64(6, 'UTC')),
    logical_bytes          UInt64 DEFAULT 0,

    -- INDEXES
    INDEX idx_metric_name metric_name TYPE bloom_filter GRANULARITY 1,
    INDEX idx_session_id session_id TYPE bloom_filter GRANULARITY 1,
    -- The datapoint lookups a confirmation makes. The sorting key puts the metric name and time before the
    -- datapoint, so without this a lookup read every granule of its project; measured over a million points,
    -- 123 of 123 granules without it and 3 with it, for about a byte per point.
    INDEX idx_datapoint_id datapoint_id TYPE bloom_filter GRANULARITY 1
) ENGINE = ReplacingMergeTree(ingested_at)
PARTITION BY toYYYYMM(timestamp)
ORDER BY (project_id, metric_name, toDate(timestamp), timestamp, datapoint_id)
TTL greatest(timestamp + INTERVAL 90 DAY, coalesce(hold_until, toDateTime64(0, 6, 'UTC'))) DELETE
SETTINGS index_granularity = 8192
"#
    .to_string()
}

fn otel_logs_columns() -> &'static str {
    r#"
    project_id LowCardinality(String),
    log_digest String,
    ordinal UInt32,
    timestamp DateTime64(6, 'UTC'),
    time Nullable(DateTime64(6, 'UTC')),
    observed_time Nullable(DateTime64(6, 'UTC')),
    severity_number Int32,
    severity_text LowCardinality(Nullable(String)),
    body Nullable(String) CODEC(ZSTD(3)),
    body_text Nullable(String) CODEC(ZSTD(3)),
    attributes Nullable(String) CODEC(ZSTD(3)),
    dropped_attributes_count UInt32,
    flags UInt32,
    trace_id Nullable(String),
    span_id Nullable(String),
    event_name LowCardinality(Nullable(String)),
    session_id Nullable(String),
    user_id Nullable(String),
    environment LowCardinality(Nullable(String)),
    service_name LowCardinality(Nullable(String)),
    service_version Nullable(String),
    service_namespace Nullable(String),
    service_instance_id Nullable(String),
    resource_attributes Nullable(String) CODEC(ZSTD(3)),
    scope_name Nullable(String),
    scope_version Nullable(String),
    scope_attributes Nullable(String) CODEC(ZSTD(3)),
    scope_schema_url Nullable(String),
    resource_schema_url Nullable(String),
    raw_log Nullable(String) CODEC(ZSTD(3)),
    ingested_at DateTime64(6, 'UTC') DEFAULT now64(6),
    hold_until Nullable(DateTime64(6, 'UTC')),
    logical_bytes UInt64 DEFAULT 0,
    search_indexed UInt8 DEFAULT 0,
    search_body Array(String) DEFAULT [],
    search_body_truncated UInt8 DEFAULT 0,
    search_event_name Array(String) DEFAULT [],
    search_event_name_truncated UInt8 DEFAULT 0,
    search_severity Array(String) DEFAULT [],
    search_severity_truncated UInt8 DEFAULT 0,
    search_attributes Array(String) DEFAULT [],
    search_attributes_truncated UInt8 DEFAULT 0,
    messages String DEFAULT '[]' CODEC(ZSTD(3)),
    INDEX idx_trace_id trace_id TYPE bloom_filter GRANULARITY 1,
    INDEX idx_span_id span_id TYPE bloom_filter GRANULARITY 1,
    INDEX idx_search_body search_body TYPE text(tokenizer = 'array') GRANULARITY 1,
    INDEX idx_search_event_name search_event_name TYPE text(tokenizer = 'array') GRANULARITY 1,
    INDEX idx_search_severity search_severity TYPE text(tokenizer = 'array') GRANULARITY 1,
    INDEX idx_search_attributes search_attributes TYPE text(tokenizer = 'array') GRANULARITY 1
"#
}

fn otel_logs_local_table(config: &ClickhouseConfig) -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS otel_logs_local ON CLUSTER {cluster} ({columns}) \
         ENGINE = ReplicatedReplacingMergeTree('/clickhouse/tables/{{shard}}/{db}/otel_logs', \
         '{{replica}}', ingested_at) PARTITION BY toYYYYMM(timestamp) \
         ORDER BY (project_id, log_digest, ordinal) \
         TTL greatest(timestamp + INTERVAL 90 DAY, coalesce(hold_until, toDateTime64(0, 6, 'UTC'))) DELETE SETTINGS index_granularity = 8192",
        cluster = safe_cluster_name(config),
        columns = otel_logs_columns(),
        db = config.database,
    )
}

fn otel_logs_distributed_table(config: &ClickhouseConfig) -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS otel_logs ON CLUSTER {cluster} AS otel_logs_local \
         ENGINE = Distributed('{cluster}', '{db}', 'otel_logs_local', sipHash64(project_id))",
        cluster = safe_cluster_name(config),
        db = config.database,
    )
}

fn otel_logs_single_table() -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS otel_logs ({}) \
         ENGINE = ReplacingMergeTree(ingested_at) PARTITION BY toYYYYMM(timestamp) \
         ORDER BY (project_id, log_digest, ordinal) \
         TTL greatest(timestamp + INTERVAL 90 DAY, coalesce(hold_until, toDateTime64(0, 6, 'UTC'))) DELETE SETTINGS index_granularity = 8192",
        otel_logs_columns()
    )
}

/// Generate all schema statements for given config
pub fn generate_schema(config: &ClickhouseConfig) -> Vec<String> {
    let mut statements = Vec::new();

    // Schema version table
    statements.push(schema_version_table(config));
    // Where the cross-partition consistency check records what it found and how far it has read. Two
    // statements in distributed mode: the local table and the `Distributed` front end that makes the report
    // deployment-wide rather than per-shard.
    statements.extend(consistency_tables(config));

    if config.distributed {
        // Distributed mode: create local tables first, then distributed tables
        statements.push(otel_spans_local_table(config));
        statements.push(otel_spans_distributed_table(config));
        statements.push(otel_metrics_local_table(config));
        statements.push(otel_metrics_distributed_table(config));
        statements.push(otel_logs_local_table(config));
        statements.push(otel_logs_distributed_table(config));
        statements.extend(raw_tables(config));
    } else {
        // Single-node mode
        statements.push(otel_spans_single_table());
        statements.push(otel_metrics_single_table());
        statements.push(otel_logs_single_table());
        statements.extend(raw_tables(config));
    }

    statements.extend(tenant_row_policies(config));
    statements
}

/// Row policies live on the physical tables. In distributed mode the front table only routes a
/// query; each shard applies this policy to its own `_local` table.
pub fn tenant_row_policies(config: &ClickhouseConfig) -> Vec<String> {
    let local = local_table_suffix(config);
    let on_cluster = get_on_cluster_clause(config);
    let database = database_identifier(config);
    [
        "otel_spans",
        "otel_metrics",
        "otel_logs",
        "otel_raw",
        "otel_raw_pending",
        "otel_raw_traces",
        "span_partition_anomalies",
    ]
    .into_iter()
    .map(|table| {
        format!(
            "CREATE ROW POLICY OR REPLACE sideseat_tenant_filter \
             ON {database}.{table}{local}{on_cluster} USING {TENANT_POLICY_EXPRESSION} TO ALL"
        )
    })
    .collect()
}

/// The table to insert into: the `Distributed` front end, which is the only thing that shards.
///
/// Its sharding expression places every project consistently. Writing to `_local` would bypass routing,
/// allowing duplicate revisions on different shards behind a load balancer or concentrating all data on one
/// node behind a fixed endpoint. Reads use the distributed table; mutations use `_local` with `ON CLUSTER`.
pub fn get_insert_table(_config: &ClickhouseConfig, base_name: &str) -> String {
    base_name.to_string()
}

/// Get the table name to query from (always the main table name)
pub fn get_query_table<'a>(_config: &ClickhouseConfig, base_name: &'a str) -> &'a str {
    base_name
}

/// Get the table name for DELETE operations (local table for distributed mode)
///
/// In distributed mode, DELETE mutations must be executed on local tables
/// because ALTER TABLE DELETE doesn't propagate through Distributed tables.
pub fn get_delete_table(config: &ClickhouseConfig, base_name: &str) -> String {
    if config.distributed {
        format!("{}_local", base_name)
    } else {
        base_name.to_string()
    }
}

/// Get the ON CLUSTER clause for DDL/mutation operations
///
/// In distributed mode, mutations need ON CLUSTER to execute on all nodes.
/// The suffix naming the table that holds the data: `_local` in distributed mode, nothing otherwise.
pub fn local_table_suffix(config: &ClickhouseConfig) -> &'static str {
    if config.distributed { "_local" } else { "" }
}

pub fn get_on_cluster_clause(config: &ClickhouseConfig) -> String {
    if config.distributed {
        format!(" ON CLUSTER {}", safe_cluster_name(config))
    } else {
        String::new()
    }
}

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;
