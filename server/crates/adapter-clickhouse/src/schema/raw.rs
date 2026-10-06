//! The raw-record table: every received export as an SSR1 record, the authority the other tables are derived
//! from.
//!
//! `ReplacingMergeTree` versioned by `version`: a deletion that removes part of a record writes the rewrite
//! as a higher version of the same `(project_id, raw_id)`. Its lifetime follows the rows derived from it -
//! the same 90-day TTL, held by `hold_until` like them - and records no row names are deleted by the raw
//! sweep before then.

use sideseat_core::config::ClickhouseConfig;

use super::safe_cluster_name;

const COLUMNS: &str = "
    project_id   LowCardinality(String),
    raw_id       String,
    signal       LowCardinality(String),
    received_at  DateTime64(6, 'UTC') CODEC(DoubleDelta, ZSTD(1)),
    rewritten    UInt8 DEFAULT 0,
    version      UInt32 DEFAULT 0,
    hold_until   Nullable(DateTime64(6, 'UTC')),
    record       String CODEC(ZSTD(3))";

const TTL: &str = "TTL greatest(received_at + INTERVAL 90 DAY, \
                   coalesce(hold_until, toDateTime64(0, 6, 'UTC'))) DELETE";

pub fn raw_tables(config: &ClickhouseConfig) -> Vec<String> {
    if !config.distributed {
        return vec![format!(
            "CREATE TABLE IF NOT EXISTS otel_raw ({COLUMNS}) \
             ENGINE = ReplacingMergeTree(version) PARTITION BY toYYYYMM(received_at) \
             ORDER BY (project_id, raw_id) {TTL}"
        )];
    }
    let cluster = safe_cluster_name(config);
    let db = &config.database;
    vec![
        format!(
            "CREATE TABLE IF NOT EXISTS otel_raw_local ON CLUSTER {cluster} ({COLUMNS}) \
             ENGINE = ReplicatedReplacingMergeTree('/clickhouse/tables/{{shard}}/{db}/otel_raw', '{{replica}}', version) \
             PARTITION BY toYYYYMM(received_at) ORDER BY (project_id, raw_id) {TTL}"
        ),
        // Sharded by project like the span table, so a project's records and its spans share a shard.
        format!(
            "CREATE TABLE IF NOT EXISTS otel_raw ON CLUSTER {cluster} AS otel_raw_local \
             ENGINE = Distributed('{cluster}', '{db}', 'otel_raw_local', sipHash64(project_id))"
        ),
    ]
}
