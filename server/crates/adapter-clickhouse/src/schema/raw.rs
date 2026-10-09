//! The raw-record table, every received export as an SSR1 record, the authority the other tables are derived
//! from, and the queue of records waiting for reconciliation with their rows.
//!
//! `ReplacingMergeTree(version)`: every write of a record - a repair, a rewrite after a deletion, a record
//! restored by the reconciler - is a version of the same `(project_id, raw_id)`, and the merge keeps the latest,
//! which is also what `FINAL` reads. Not partitioned: the client resolves `FINAL` one partition at a time
//! (`do_not_merge_across_partitions_select_final`), so a record's versions must share one, and a partition by
//! receipt month split them whenever two first deliveries of one body straddled a month or a version carried
//! another receipt - each month's version was then a latest one, the content a deletion removed among them. No
//! read prunes by receipt, and expiry is by row (`signal_until`), so the months bought nothing. The lifecycle is `server/specs/RawRecordOwnership.tla`. The time to live is
//! the latest of its rows': `signal_until` is the latest span start the record carries, and the span rows
//! expire 90 days after their own start, so no row can outlive its record; `hold_until` holds it like them.
//! The trace index lives exactly as long as the record version that wrote it.

use sideseat_core::config::ClickhouseConfig;

use super::safe_cluster_name;

const COLUMNS: &str = "
    project_id   LowCardinality(String),
    raw_id       String,
    signal       LowCardinality(String),
    received_at  DateTime64(6, 'UTC') CODEC(DoubleDelta, ZSTD(1)),
    origin       LowCardinality(String),
    version      Int64,
    signal_until DateTime64(6, 'UTC'),
    hold_until   Nullable(DateTime64(6, 'UTC')),
    record       String CODEC(ZSTD(3))";

const TTL: &str = "TTL greatest(signal_until + INTERVAL 90 DAY, \
                   coalesce(hold_until, toDateTime64(0, 6, 'UTC'))) DELETE";

const TRACE_COLUMNS: &str = "
    project_id   LowCardinality(String),
    trace_id     String,
    raw_id       String,
    signal_until DateTime64(6, 'UTC'),
    hold_until   Nullable(DateTime64(6, 'UTC'))";

const PENDING_COLUMNS: &str = "
    project_id   LowCardinality(String),
    raw_id       String,
    token        String,
    enqueued_at  DateTime64(6, 'UTC')";

pub fn raw_tables(config: &ClickhouseConfig) -> Vec<String> {
    if !config.distributed {
        return vec![
            format!(
                "CREATE TABLE IF NOT EXISTS otel_raw ({COLUMNS}) \
                 ENGINE = ReplacingMergeTree(version) ORDER BY (project_id, raw_id) {TTL}"
            ),
            format!(
                "CREATE TABLE IF NOT EXISTS otel_raw_traces ({TRACE_COLUMNS}) \
                 ENGINE = ReplacingMergeTree ORDER BY (project_id, trace_id, raw_id) {TTL}"
            ),
            format!(
                "CREATE TABLE IF NOT EXISTS otel_raw_pending ({PENDING_COLUMNS}) \
                 ENGINE = MergeTree ORDER BY (project_id, raw_id, token)"
            ),
        ];
    }
    let cluster = safe_cluster_name(config);
    let db = &config.database;
    vec![
        format!(
            "CREATE TABLE IF NOT EXISTS otel_raw_local ON CLUSTER {cluster} ({COLUMNS}) \
             ENGINE = ReplicatedReplacingMergeTree('/clickhouse/tables/{{shard}}/{db}/otel_raw', '{{replica}}', version) \
             ORDER BY (project_id, raw_id) {TTL}"
        ),
        // Sharded by project like the span table, so a project's records and its spans share a shard.
        format!(
            "CREATE TABLE IF NOT EXISTS otel_raw ON CLUSTER {cluster} AS otel_raw_local \
             ENGINE = Distributed('{cluster}', '{db}', 'otel_raw_local', sipHash64(project_id))"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS otel_raw_traces_local ON CLUSTER {cluster} ({TRACE_COLUMNS}) \
             ENGINE = ReplicatedReplacingMergeTree('/clickhouse/tables/{{shard}}/{db}/otel_raw_traces', '{{replica}}') \
             ORDER BY (project_id, trace_id, raw_id) {TTL}"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS otel_raw_traces ON CLUSTER {cluster} AS otel_raw_traces_local \
             ENGINE = Distributed('{cluster}', '{db}', 'otel_raw_traces_local', sipHash64(project_id))"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS otel_raw_pending_local ON CLUSTER {cluster} ({PENDING_COLUMNS}) \
             ENGINE = ReplicatedMergeTree('/clickhouse/tables/{{shard}}/{db}/otel_raw_pending', '{{replica}}') \
             ORDER BY (project_id, raw_id, token)"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS otel_raw_pending ON CLUSTER {cluster} AS otel_raw_pending_local \
             ENGINE = Distributed('{cluster}', '{db}', 'otel_raw_pending_local', sipHash64(project_id))"
        ),
    ]
}
