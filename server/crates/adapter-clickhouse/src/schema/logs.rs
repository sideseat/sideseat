//! The log-record tables: one row per log record, keyed by its digest and ordinal.

use sideseat_core::config::ClickhouseConfig;

use super::safe_cluster_name;

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
    -- NULL for a record that carries no messages, and for one whose raw_log rebuilds them (domain::log_messages);
    -- stored where it does not, or where they hold a file reference a release must see.
    messages Nullable(String) CODEC(ZSTD(3)),
    -- The key the frame this record carries states, in its project's and trace's form (RequestFrames::stored_key),
    -- where its messages were read from a carrier that frames requests (CarrierRule::frames_requests); zero on every
    -- other record, which the key never is. A framed request's view reads by it within one trace, which
    -- `idx_trace_id` already narrows to that trace's granules, so no index of its own.
    frame_key UInt128 DEFAULT 0 CODEC(ZSTD(1)),
    -- The references this record's views show as a note, as a JSON array; NULL on every record without one. With
    -- raw_log, what an unstored body, body_text and attributes are rebuilt from (domain::log_views).
    noted_references Nullable(String) CODEC(ZSTD(3)),
    -- The length in bytes of the record's messages as their JSON array, stored or not; zero where it carries none.
    messages_bytes UInt32 DEFAULT 0 CODEC(ZSTD(1)),
    -- A hash of the record's messages, zero where it carries none (NormalizedLog::messages_hash): what a feed page
    -- and the read of its messages compare, so a redelivery that changes the messages at the same instant is seen.
    messages_hash UInt64 DEFAULT 0 CODEC(ZSTD(1)),
    INDEX idx_trace_id trace_id TYPE bloom_filter GRANULARITY 1,
    INDEX idx_span_id span_id TYPE bloom_filter GRANULARITY 1,
    INDEX idx_search_body search_body TYPE text(tokenizer = 'array') GRANULARITY 1,
    INDEX idx_search_event_name search_event_name TYPE text(tokenizer = 'array') GRANULARITY 1,
    INDEX idx_search_severity search_severity TYPE text(tokenizer = 'array') GRANULARITY 1,
    INDEX idx_search_attributes search_attributes TYPE text(tokenizer = 'array') GRANULARITY 1
"#
}

/// The log partition: the month of the record's own time, else its observed time - both the producer's, so every
/// delivery of a record lands in one partition - and one partition for records with neither.
///
/// Not the month of `timestamp`, which falls back to the receipt for a record with no time of its own: the client
/// resolves `FINAL` a partition at a time, so the same record delivered again in another month was two rows, and
/// every read of it - its span's log messages among them - had it twice.
const OTEL_LOGS_PARTITION: &str =
    "toYYYYMM(coalesce(time, observed_time, toDateTime64(0, 6, 'UTC')))";

pub(super) fn otel_logs_local_table(config: &ClickhouseConfig) -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS otel_logs_local ON CLUSTER {cluster} ({columns}) \
         ENGINE = ReplicatedReplacingMergeTree('/clickhouse/tables/{{shard}}/{db}/otel_logs', \
         '{{replica}}', ingested_at) PARTITION BY {OTEL_LOGS_PARTITION} \
         ORDER BY (project_id, log_digest, ordinal) \
         TTL greatest(timestamp + INTERVAL 90 DAY, coalesce(hold_until, toDateTime64(0, 6, 'UTC'))) DELETE SETTINGS index_granularity = 8192",
        cluster = safe_cluster_name(config),
        columns = otel_logs_columns(),
        db = config.database,
    )
}

pub(super) fn otel_logs_distributed_table(config: &ClickhouseConfig) -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS otel_logs ON CLUSTER {cluster} AS otel_logs_local \
         ENGINE = Distributed('{cluster}', '{db}', 'otel_logs_local', sipHash64(project_id))",
        cluster = safe_cluster_name(config),
        db = config.database,
    )
}

pub(super) fn otel_logs_single_table() -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS otel_logs ({}) \
         ENGINE = ReplacingMergeTree(ingested_at) PARTITION BY {OTEL_LOGS_PARTITION} \
         ORDER BY (project_id, log_digest, ordinal) \
         TTL greatest(timestamp + INTERVAL 90 DAY, coalesce(hold_until, toDateTime64(0, 6, 'UTC'))) DELETE SETTINGS index_granularity = 8192",
        otel_logs_columns()
    )
}
