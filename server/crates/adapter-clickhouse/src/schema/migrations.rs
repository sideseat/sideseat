/// One schema migration.
///
/// Every statement has `{on_cluster}` replaced with the ON CLUSTER clause - required for DDL in
/// distributed mode, empty otherwise - and `{local}` with the suffix that names the table holding the
/// data (`_local` in distributed mode, nothing in single-node mode).
///
/// A fresh database is created directly at [`super::SCHEMA_VERSION`] by the initial schema, so entries exist
/// solely for databases written by older builds. `migrations_cover_every_version` requires one entry for
/// every supported upgrade step.
pub struct Migration {
    pub version: i32,
    pub name: &'static str,
    /// A query returning one row when the migration still has work to do, and no rows when it is
    /// already applied. Checked before `statements` run.
    ///
    /// Not every schema change can be phrased idempotently. `ALTER TABLE ... ADD COLUMN, MODIFY ORDER
    /// BY` is the case in hand: ClickHouse permits a sorting key to be extended only by a column added
    /// in the *same* statement, so re-running it after it has succeeded is an error rather than a
    /// no-op - and a migration whose statements succeed while the version record fails would then leave
    /// a database that can never start again. A precondition makes retrying safe without requiring
    /// every statement to be individually idempotent, which is the property that is actually hard to
    /// keep by hand.
    pub precondition: Option<&'static str>,
    /// Applied in order, against the table that holds the data.
    pub statements: &'static [&'static str],
    /// Applied only in distributed mode, after `statements`.
    ///
    /// A `Distributed` table is created `AS otel_x_local`, which copies the structure once; it does not
    /// track later changes. So a column added to the local table has to be added to the distributed
    /// front end too, and in single-node mode there is no such table and nothing to do.
    pub distributed_statements: &'static [&'static str],
}

/// The v2 → v3 rebuild: a sorting key that is a function of identity alone, and a version column on
/// metrics.
///
/// **What this fixes, and what it does not.** A `ReplacingMergeTree` identifies duplicates *by the sorting
/// key*, so with `toDate(timestamp_start)` in it a corrected re-delivery crossing **midnight UTC** got a
/// different key and `FINAL` returned both revisions. Making the key identity alone collapses that case.
/// A correction crossing a **month** boundary is **not** fixed: `PARTITION BY toYYYYMM(timestamp_start)`
/// stays, parts in different partitions never merge, and `do_not_merge_across_partitions_select_final`
/// (`mod.rs`) makes `FINAL` per-partition — so both revisions remain visible. That residual is stated at the
/// setting rather than repaired here; it is a reported hole, not a fixed one.
///
/// **Why a rebuild and not an `ALTER`.** `MODIFY ORDER BY` is *rejected* on these tables — "Primary key
/// must be a prefix of the sorting key" — because the existing implicit primary key contains
/// `toDate(timestamp_start)`, and it is metadata-only in any case, so it would not re-sort the parts
/// that already exist. The metrics change needs a rebuild too: an engine's version argument cannot be
/// altered. Both tables are therefore recreated, copied, and swapped in one migration, which is also
/// why they share a version: split across two, whichever landed first would record v3 and the other
/// would never run again on that database.
///
/// **`CREATE TABLE … AS <old>` copies the columns *and* the data-skipping indexes**, verified against
/// 25.8, so the migration does not restate a hundred-column DDL that would then drift from the fresh
/// schema above.
///
/// **What existing metric rows get for a version.** V2 has no such column, so the copy defines a
/// baseline: `FROM otel_metrics FINAL` selects the **pre-migration winner** — defined, because an
/// unversioned `ReplacingMergeTree` keeps the most recently inserted row — and stamps it with the
/// epoch. Two ways to get this wrong, both avoided: giving duplicate historical rows *equal*
/// synthesized versions leaves the winner free to flip at the next merge, and stamping *migration time*
/// would outrank the first legitimate clock-regressed update that follows.
///
/// **Stated residual: a released row and a later correction of the same datapoint both survive.** V2 has no
/// `datapoint_id`, so the migration gives existing rows the column's default of `''`, while a post-upgrade
/// delivery of that same OTLP datapoint carries a real digest. `datapoint_id` is in the new sorting key, so the
/// two keys differ and `FINAL` returns both - a correction *adding to* the measurement it was meant to replace.
/// Reproduced against 25.8: one released row of 1, one correction of 2, `FINAL` gives two rows summing to 3.
///
/// It cannot be backfilled. The id is a digest over OTLP attribute values **with their protobuf variants
/// preserved** (`server/crates/ingestion/src/metrics/identity.rs`) - which is the whole reason it is not forgeable - and no SQL
/// expression reproduces that from stored columns. The three alternatives are worse: deleting the released rows
/// is data loss the operator has not asked for; leaving `toDate(timestamp)`-style keys is the labelled-metric
/// collapse this migration exists to fix; and hiding an empty-id row whenever an identified one appears for the
/// same `(metric, timestamp)` would suppress a genuine unattributed aggregate on the arrival of one unrelated
/// series.
///
/// So it over-reports rather than under-reports, which is the side this codebase takes when the fact is
/// unavailable - and it is **counted rather than merely described**: the count is taken straight after the
/// rebuild, where it is free because the table has just been rewritten, and reported with the remedy. Pinned by
/// `a_released_metric_row_and_its_correction_both_survive`, so nobody reads the fix as covering it.
///
/// **A leftover `_v3` table means the work is not finished.** After `EXCHANGE TABLES` the *old* table wears
/// the `_v3` name and is dropped next; a crash in between leaves it there while both the sorting key and the
/// version column already look correct, so a precondition asking only about those reports the migration
/// applied and the old full-size table is never reclaimed - silently doubling the storage of the two largest
/// tables. Naming those tables in the precondition makes the re-run finish the job, which is safe because
/// every statement is idempotent.
///
/// Spans are copied **without** `FINAL`: their engine is already versioned, so the rebuild is purely a
/// re-sort and every revision is preserved, leaving deduplication where it belongs — at read time.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 3,
        name: "identity_sorting_key_and_metric_version",
        // Asks the tables themselves whether the work is due - a row means "still to do". A leftover
        // `_v3` table would have been the wrong signal: it is absent *before* the migration, so the
        // statements would have been skipped on exactly the databases that need them.
        precondition: Some(
            "SELECT 1 FROM system.tables WHERE database = currentDatabase() AND ( \
           (name = 'otel_spans{local}' AND position(sorting_key, 'toDate(') > 0) \
           OR (name = 'otel_metrics{local}' AND engine_full NOT LIKE '%ingested_at%') \
           OR name IN ('otel_spans_v3{local}', 'otel_metrics_v3{local}') \
         ) LIMIT 1",
        ),
        statements: &[
            // -- first, the columns a *genuine* v2 database does not have -----------------------------------
            //
            // `MIN_UPGRADABLE_FROM` is 2 and the released v1.0.13 schema declares version 2 - but five metrics
            // columns and two span columns were added to the fresh schema *after* that release without the
            // version ever being bumped. So "version 2" names several physically different schemas, and the
            // rebuild below would fail on a real one: its `ORDER BY (…, datapoint_id)` is `UNKNOWN_IDENTIFIER`
            // when the column is absent, which is a startup failure no retry can clear.
            //
            // Idempotent, so a database that already has them - every one created by a recent build - is
            // unaffected. This is what makes `MIN_UPGRADABLE_FROM = 2` a true statement rather than an intention.
            "ALTER TABLE otel_metrics{local}{on_cluster} ADD COLUMN IF NOT EXISTS datapoint_id String",
            "ALTER TABLE otel_metrics{local}{on_cluster} ADD COLUMN IF NOT EXISTS scope_attributes Nullable(String)",
            "ALTER TABLE otel_metrics{local}{on_cluster} ADD COLUMN IF NOT EXISTS scope_schema_url Nullable(String)",
            "ALTER TABLE otel_metrics{local}{on_cluster} ADD COLUMN IF NOT EXISTS resource_schema_url Nullable(String)",
            "ALTER TABLE otel_metrics{local}{on_cluster} ADD COLUMN IF NOT EXISTS exemplars Nullable(String) CODEC(ZSTD(3))",
            "ALTER TABLE otel_spans{local}{on_cluster} ADD COLUMN IF NOT EXISTS scope_name Nullable(String) CODEC(ZSTD(1))",
            "ALTER TABLE otel_spans{local}{on_cluster} ADD COLUMN IF NOT EXISTS scope_version Nullable(String) CODEC(ZSTD(1))",
            // The skip index the consistency check needs, added **here** rather than after the rebuild: the
            // replacement is created `AS otel_spans{local}`, which copies data-skipping indexes, so adding it
            // before the copy is what gets it onto the table that survives. Added after the `EXCHANGE` it would
            // have landed on the table about to be dropped.
            //
            // No backfill statement follows it. `ALTER TABLE ... ADD INDEX` is metadata-only - it does not build
            // the index over parts that already exist - but the rebuild below rewrites every part through
            // `INSERT ... SELECT`, so the index is populated as a side effect of the copy this migration was
            // already doing. A migration that only added the index would need `MATERIALIZE INDEX`.
            "ALTER TABLE otel_spans{local}{on_cluster} \
         ADD INDEX IF NOT EXISTS idx_ingested_at ingested_at TYPE minmax GRANULARITY 1",
            // -- spans: re-sort on identity ------------------------------------------------------------
            "DROP TABLE IF EXISTS otel_spans_v3{local}{on_cluster} SYNC",
            "CREATE TABLE otel_spans_v3{local}{on_cluster} AS otel_spans{local} \
         ENGINE = {replacement_engine} \
         PARTITION BY toYYYYMM(timestamp_start) \
         ORDER BY (project_id, trace_id, span_id) \
         TTL timestamp_start + INTERVAL 90 DAY DELETE \
         SETTINGS index_granularity = 8192, merge_with_ttl_timeout = 3600",
            "INSERT INTO otel_spans_v3{local} SELECT * FROM otel_spans{local}",
            "EXCHANGE TABLES otel_spans{local} AND otel_spans_v3{local}{on_cluster}",
            "DROP TABLE IF EXISTS otel_spans_v3{local}{on_cluster} SYNC",
            // -- metrics: a version column, then re-engine ---------------------------------------------
            // The baseline is the column's DEFAULT, not a value the copy stamps on. Rows written before the
            // column existed therefore read as the epoch - below any real `ingested_at` - without the copy
            // having to distinguish them, which is what makes a re-run safe: a `REPLACE (epoch AS
            // ingested_at)` in the copy would have reset versions the first run had already migrated.
            "ALTER TABLE otel_metrics{local}{on_cluster} \
         ADD COLUMN IF NOT EXISTS ingested_at DateTime64(6, 'UTC') DEFAULT toDateTime64(0, 6, 'UTC')",
            "DROP TABLE IF EXISTS otel_metrics_v3{local}{on_cluster} SYNC",
            "CREATE TABLE otel_metrics_v3{local}{on_cluster} AS otel_metrics{local} \
         ENGINE = {replacement_engine} \
         PARTITION BY toYYYYMM(timestamp) \
         ORDER BY (project_id, metric_name, toDate(timestamp), timestamp, datapoint_id) \
         TTL timestamp + INTERVAL 90 DAY DELETE \
         SETTINGS index_granularity = 8192",
            // Align the replacement's DEFAULT with the fresh schema *before* it holds data, so an upgraded
            // database and a fresh one are metadata-identical. Applied here rather than to the old table,
            // where it would have made the epoch baseline unavailable to the copy below.
            "ALTER TABLE otel_metrics_v3{local}{on_cluster} \
         MODIFY COLUMN ingested_at DateTime64(6, 'UTC') DEFAULT now64(6)",
            // `FINAL` on the source is the pre-migration winner: defined for the unversioned engine (the
            // most recently inserted row) and, on a re-run against the already-versioned one, the highest
            // `ingested_at`. Either way exactly one row per datapoint, carrying its real version.
            "INSERT INTO otel_metrics_v3{local} SELECT * FROM otel_metrics{local} FINAL",
            "EXCHANGE TABLES otel_metrics{local} AND otel_metrics_v3{local}{on_cluster}",
            "DROP TABLE IF EXISTS otel_metrics_v3{local}{on_cluster} SYNC",
        ],
        // `Distributed` front ends are created `AS otel_x_local`, which copies the structure once and does
        // not track later changes - so the metrics column has to be added there too. The spans change is
        // confined to the local table's sorting key, which a front end does not carry.
        distributed_statements: &[
            "ALTER TABLE otel_metrics{on_cluster} ADD COLUMN IF NOT EXISTS datapoint_id String",
            "ALTER TABLE otel_metrics{on_cluster} ADD COLUMN IF NOT EXISTS scope_attributes Nullable(String)",
            "ALTER TABLE otel_metrics{on_cluster} ADD COLUMN IF NOT EXISTS scope_schema_url Nullable(String)",
            "ALTER TABLE otel_metrics{on_cluster} ADD COLUMN IF NOT EXISTS resource_schema_url Nullable(String)",
            "ALTER TABLE otel_metrics{on_cluster} ADD COLUMN IF NOT EXISTS exemplars Nullable(String) CODEC(ZSTD(3))",
            "ALTER TABLE otel_spans{on_cluster} ADD COLUMN IF NOT EXISTS scope_name Nullable(String) CODEC(ZSTD(1))",
            "ALTER TABLE otel_spans{on_cluster} ADD COLUMN IF NOT EXISTS scope_version Nullable(String) CODEC(ZSTD(1))",
            "ALTER TABLE otel_metrics{on_cluster} \
         ADD COLUMN IF NOT EXISTS ingested_at DateTime64(6, 'UTC') DEFAULT now64(6)",
        ],
    },
    Migration {
        version: 4,
        name: "otel_logs",
        precondition: Some(
            "SELECT 1 WHERE \
         NOT EXISTS (SELECT 1 FROM system.tables \
             WHERE database = currentDatabase() AND name = 'otel_logs{local}') \
         OR NOT EXISTS (SELECT 1 FROM system.columns \
             WHERE database = currentDatabase() AND table = 'otel_spans{local}' \
             AND name = 'content_digest') \
         OR NOT EXISTS (SELECT 1 FROM system.columns \
             WHERE database = currentDatabase() AND table = 'otel_metrics{local}' \
             AND name = 'content_digest')",
        ),
        statements: &[
            "ALTER TABLE otel_spans{local}{on_cluster} \
         ADD COLUMN IF NOT EXISTS content_digest String DEFAULT ''",
            "ALTER TABLE otel_metrics{local}{on_cluster} \
         ADD COLUMN IF NOT EXISTS content_digest String DEFAULT ''",
            "CREATE TABLE IF NOT EXISTS otel_logs{local}{on_cluster} (
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
            INDEX idx_trace_id trace_id TYPE bloom_filter GRANULARITY 1,
            INDEX idx_span_id span_id TYPE bloom_filter GRANULARITY 1
         ) ENGINE = {replacement_engine}
         PARTITION BY toYYYYMM(timestamp)
         ORDER BY (project_id, log_digest, ordinal)
         TTL timestamp + INTERVAL 90 DAY DELETE
         SETTINGS index_granularity = 8192",
        ],
        distributed_statements: &[
            "ALTER TABLE otel_spans{on_cluster} \
         ADD COLUMN IF NOT EXISTS content_digest String DEFAULT ''",
            "ALTER TABLE otel_metrics{on_cluster} \
         ADD COLUMN IF NOT EXISTS content_digest String DEFAULT ''",
            "CREATE TABLE IF NOT EXISTS otel_logs{on_cluster} AS otel_logs_local
         ENGINE = Distributed('{cluster}', '{database}', 'otel_logs_local', sipHash64(project_id))",
        ],
    },
    Migration {
        version: 5,
        name: "logical_bytes_and_legal_hold",
        precondition: Some(
            "SELECT 1 WHERE \
         NOT EXISTS (SELECT 1 FROM system.columns \
             WHERE database = currentDatabase() AND table = 'otel_spans{local}' AND name = 'hold_until') \
         OR NOT EXISTS (SELECT 1 FROM system.columns \
             WHERE database = currentDatabase() AND table = 'otel_metrics{local}' AND name = 'hold_until') \
         OR NOT EXISTS (SELECT 1 FROM system.columns \
             WHERE database = currentDatabase() AND table = 'otel_logs{local}' AND name = 'hold_until')",
        ),
        statements: &[
            "ALTER TABLE otel_spans{local}{on_cluster} ADD COLUMN IF NOT EXISTS hold_until Nullable(DateTime64(6, 'UTC'))",
            "ALTER TABLE otel_spans{local}{on_cluster} ADD COLUMN IF NOT EXISTS logical_bytes UInt64 DEFAULT 0",
            "ALTER TABLE otel_metrics{local}{on_cluster} ADD COLUMN IF NOT EXISTS hold_until Nullable(DateTime64(6, 'UTC'))",
            "ALTER TABLE otel_metrics{local}{on_cluster} ADD COLUMN IF NOT EXISTS logical_bytes UInt64 DEFAULT 0",
            "ALTER TABLE otel_logs{local}{on_cluster} ADD COLUMN IF NOT EXISTS hold_until Nullable(DateTime64(6, 'UTC'))",
            "ALTER TABLE otel_logs{local}{on_cluster} ADD COLUMN IF NOT EXISTS logical_bytes UInt64 DEFAULT 0",
            "ALTER TABLE otel_spans{local}{on_cluster} MODIFY TTL greatest(timestamp_start + INTERVAL 90 DAY, coalesce(hold_until, toDateTime64(0, 6, 'UTC'))) DELETE",
            "ALTER TABLE otel_metrics{local}{on_cluster} MODIFY TTL greatest(timestamp + INTERVAL 90 DAY, coalesce(hold_until, toDateTime64(0, 6, 'UTC'))) DELETE",
            "ALTER TABLE otel_logs{local}{on_cluster} MODIFY TTL greatest(timestamp + INTERVAL 90 DAY, coalesce(hold_until, toDateTime64(0, 6, 'UTC'))) DELETE",
        ],
        distributed_statements: &[
            "ALTER TABLE otel_spans{on_cluster} ADD COLUMN IF NOT EXISTS hold_until Nullable(DateTime64(6, 'UTC'))",
            "ALTER TABLE otel_spans{on_cluster} ADD COLUMN IF NOT EXISTS logical_bytes UInt64 DEFAULT 0",
            "ALTER TABLE otel_metrics{on_cluster} ADD COLUMN IF NOT EXISTS hold_until Nullable(DateTime64(6, 'UTC'))",
            "ALTER TABLE otel_metrics{on_cluster} ADD COLUMN IF NOT EXISTS logical_bytes UInt64 DEFAULT 0",
            "ALTER TABLE otel_logs{on_cluster} ADD COLUMN IF NOT EXISTS hold_until Nullable(DateTime64(6, 'UTC'))",
            "ALTER TABLE otel_logs{on_cluster} ADD COLUMN IF NOT EXISTS logical_bytes UInt64 DEFAULT 0",
        ],
    },
    Migration {
        version: 6,
        name: "search_term_arrays",
        precondition: Some(
            "SELECT 1 WHERE NOT EXISTS (SELECT 1 FROM system.columns \
             WHERE database = currentDatabase() AND table = 'otel_spans{local}' \
             AND name = 'search_indexed')",
        ),
        statements: &[
            "ALTER TABLE otel_spans{local}{on_cluster} \
             ADD COLUMN IF NOT EXISTS search_indexed UInt8 DEFAULT 0, \
             ADD COLUMN IF NOT EXISTS search_prompt Array(String) DEFAULT [], \
             ADD COLUMN IF NOT EXISTS search_prompt_truncated UInt8 DEFAULT 0, \
             ADD COLUMN IF NOT EXISTS search_completion Array(String) DEFAULT [], \
             ADD COLUMN IF NOT EXISTS search_completion_truncated UInt8 DEFAULT 0, \
             ADD COLUMN IF NOT EXISTS search_tool_name Array(String) DEFAULT [], \
             ADD COLUMN IF NOT EXISTS search_tool_name_truncated UInt8 DEFAULT 0, \
             ADD COLUMN IF NOT EXISTS search_tool_args Array(String) DEFAULT [], \
             ADD COLUMN IF NOT EXISTS search_tool_args_truncated UInt8 DEFAULT 0, \
             ADD COLUMN IF NOT EXISTS search_error Array(String) DEFAULT [], \
             ADD COLUMN IF NOT EXISTS search_error_truncated UInt8 DEFAULT 0, \
             ADD COLUMN IF NOT EXISTS search_span_name Array(String) DEFAULT [], \
             ADD COLUMN IF NOT EXISTS search_span_name_truncated UInt8 DEFAULT 0",
            "ALTER TABLE otel_logs{local}{on_cluster} \
             ADD COLUMN IF NOT EXISTS search_indexed UInt8 DEFAULT 0, \
             ADD COLUMN IF NOT EXISTS search_body Array(String) DEFAULT [], \
             ADD COLUMN IF NOT EXISTS search_body_truncated UInt8 DEFAULT 0, \
             ADD COLUMN IF NOT EXISTS search_event_name Array(String) DEFAULT [], \
             ADD COLUMN IF NOT EXISTS search_event_name_truncated UInt8 DEFAULT 0, \
             ADD COLUMN IF NOT EXISTS search_severity Array(String) DEFAULT [], \
             ADD COLUMN IF NOT EXISTS search_severity_truncated UInt8 DEFAULT 0, \
             ADD COLUMN IF NOT EXISTS search_attributes Array(String) DEFAULT [], \
             ADD COLUMN IF NOT EXISTS search_attributes_truncated UInt8 DEFAULT 0",
            "ALTER TABLE otel_spans{local}{on_cluster} \
             ADD INDEX IF NOT EXISTS idx_search_prompt search_prompt TYPE text(tokenizer = 'array') GRANULARITY 1, \
             ADD INDEX IF NOT EXISTS idx_search_completion search_completion TYPE text(tokenizer = 'array') GRANULARITY 1, \
             ADD INDEX IF NOT EXISTS idx_search_tool_name search_tool_name TYPE text(tokenizer = 'array') GRANULARITY 1, \
             ADD INDEX IF NOT EXISTS idx_search_tool_args search_tool_args TYPE text(tokenizer = 'array') GRANULARITY 1, \
             ADD INDEX IF NOT EXISTS idx_search_error search_error TYPE text(tokenizer = 'array') GRANULARITY 1, \
             ADD INDEX IF NOT EXISTS idx_search_span_name search_span_name TYPE text(tokenizer = 'array') GRANULARITY 1",
            "ALTER TABLE otel_logs{local}{on_cluster} \
             ADD INDEX IF NOT EXISTS idx_search_body search_body TYPE text(tokenizer = 'array') GRANULARITY 1, \
             ADD INDEX IF NOT EXISTS idx_search_event_name search_event_name TYPE text(tokenizer = 'array') GRANULARITY 1, \
             ADD INDEX IF NOT EXISTS idx_search_severity search_severity TYPE text(tokenizer = 'array') GRANULARITY 1, \
             ADD INDEX IF NOT EXISTS idx_search_attributes search_attributes TYPE text(tokenizer = 'array') GRANULARITY 1",
        ],
        distributed_statements: &[
            "ALTER TABLE otel_spans{on_cluster} \
             ADD COLUMN IF NOT EXISTS search_indexed UInt8 DEFAULT 0, \
             ADD COLUMN IF NOT EXISTS search_prompt Array(String) DEFAULT [], \
             ADD COLUMN IF NOT EXISTS search_prompt_truncated UInt8 DEFAULT 0, \
             ADD COLUMN IF NOT EXISTS search_completion Array(String) DEFAULT [], \
             ADD COLUMN IF NOT EXISTS search_completion_truncated UInt8 DEFAULT 0, \
             ADD COLUMN IF NOT EXISTS search_tool_name Array(String) DEFAULT [], \
             ADD COLUMN IF NOT EXISTS search_tool_name_truncated UInt8 DEFAULT 0, \
             ADD COLUMN IF NOT EXISTS search_tool_args Array(String) DEFAULT [], \
             ADD COLUMN IF NOT EXISTS search_tool_args_truncated UInt8 DEFAULT 0, \
             ADD COLUMN IF NOT EXISTS search_error Array(String) DEFAULT [], \
             ADD COLUMN IF NOT EXISTS search_error_truncated UInt8 DEFAULT 0, \
             ADD COLUMN IF NOT EXISTS search_span_name Array(String) DEFAULT [], \
             ADD COLUMN IF NOT EXISTS search_span_name_truncated UInt8 DEFAULT 0",
            "ALTER TABLE otel_logs{on_cluster} \
             ADD COLUMN IF NOT EXISTS search_indexed UInt8 DEFAULT 0, \
             ADD COLUMN IF NOT EXISTS search_body Array(String) DEFAULT [], \
             ADD COLUMN IF NOT EXISTS search_body_truncated UInt8 DEFAULT 0, \
             ADD COLUMN IF NOT EXISTS search_event_name Array(String) DEFAULT [], \
             ADD COLUMN IF NOT EXISTS search_event_name_truncated UInt8 DEFAULT 0, \
             ADD COLUMN IF NOT EXISTS search_severity Array(String) DEFAULT [], \
             ADD COLUMN IF NOT EXISTS search_severity_truncated UInt8 DEFAULT 0, \
             ADD COLUMN IF NOT EXISTS search_attributes Array(String) DEFAULT [], \
             ADD COLUMN IF NOT EXISTS search_attributes_truncated UInt8 DEFAULT 0",
        ],
    },
    Migration {
        version: 7,
        name: "tenant_row_policies",
        precondition: None,
        statements: &[
            "CREATE ROW POLICY OR REPLACE sideseat_tenant_filter \
             ON {database_identifier}.otel_spans{local}{on_cluster} \
             USING project_id = getSettingOrDefault('SQL_sideseat_project_id', '') \
                OR getSettingOrDefault('SQL_sideseat_maintenance', 0) = 1 TO ALL",
            "CREATE ROW POLICY OR REPLACE sideseat_tenant_filter \
             ON {database_identifier}.otel_metrics{local}{on_cluster} \
             USING project_id = getSettingOrDefault('SQL_sideseat_project_id', '') \
                OR getSettingOrDefault('SQL_sideseat_maintenance', 0) = 1 TO ALL",
            "CREATE ROW POLICY OR REPLACE sideseat_tenant_filter \
             ON {database_identifier}.otel_logs{local}{on_cluster} \
             USING project_id = getSettingOrDefault('SQL_sideseat_project_id', '') \
                OR getSettingOrDefault('SQL_sideseat_maintenance', 0) = 1 TO ALL",
            "CREATE ROW POLICY OR REPLACE sideseat_tenant_filter \
             ON {database_identifier}.span_partition_anomalies{local}{on_cluster} \
             USING project_id = getSettingOrDefault('SQL_sideseat_project_id', '') \
                OR getSettingOrDefault('SQL_sideseat_maintenance', 0) = 1 TO ALL",
        ],
        distributed_statements: &[],
    },
    // The messages a log record carries for the span it names, derived at ingest and joined at read time.
    // Existing rows take the empty array: nothing was read from them, which is what it says.
    Migration {
        version: 8,
        name: "log_messages",
        precondition: Some(
            "SELECT 1 WHERE NOT EXISTS (SELECT 1 FROM system.columns \
             WHERE database = currentDatabase() AND table = 'otel_logs{local}' \
             AND name = 'messages')",
        ),
        statements: &["ALTER TABLE otel_logs{local}{on_cluster} \
             ADD COLUMN IF NOT EXISTS messages String DEFAULT '[]' CODEC(ZSTD(3))"],
        distributed_statements: &["ALTER TABLE otel_logs{on_cluster} \
             ADD COLUMN IF NOT EXISTS messages String DEFAULT '[]' CODEC(ZSTD(3))"],
    },
];
