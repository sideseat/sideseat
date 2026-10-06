use super::*;

fn default_config() -> ClickhouseConfig {
    ClickhouseConfig {
        url: "http://localhost:8123".to_string(),
        database: "sideseat".to_string(),
        user: None,
        password: None,
        timeout_secs: 30,
        compression: true,
        async_insert: true,
        wait_for_async_insert: false,
        cluster: None,
        distributed: false,
        insert_quorum: 0,
    }
}

#[test]
#[allow(clippy::assertions_on_constants)]
fn test_schema_version_is_positive() {
    assert!(SCHEMA_VERSION > 0);
}

#[test]
fn test_generate_schema_single_node() {
    let config = default_config();
    let statements = generate_schema(&config);

    // Six tables followed by one policy for each project-scoped physical table.
    assert_eq!(statements.len(), 11);
    // Indexed by name rather than by position, because a count plus a positional assertion is what made
    // adding a table here a two-test edit with a silent window in between.
    let spans = statements
        .iter()
        .find(|s| s.contains("CREATE TABLE IF NOT EXISTS otel_spans"))
        .expect("the span table is generated");

    // Should use ReplacingMergeTree (not Replicated)
    assert!(spans.contains("ReplacingMergeTree"));
    assert!(!spans.contains("ReplicatedReplacingMergeTree"));
    assert!(!spans.contains("ON CLUSTER"));
}

#[test]
fn test_generate_schema_distributed() {
    let config = ClickhouseConfig {
        cluster: Some("test_cluster".to_string()),
        distributed: true,
        ..default_config()
    };
    let statements = generate_schema(&config);

    // Fifteen tables followed by seven policies on the physical `_local` tables.
    assert_eq!(statements.len(), 22);
    // The anomaly table needs a front end in distributed mode or the report is per-shard: a pass on shard A
    // records there and a read reaching shard B returns nothing.
    assert!(
        statements
            .iter()
            .any(|s| s.contains("span_partition_anomalies ON CLUSTER")
                && s.contains("ENGINE = Distributed")),
        "the anomaly table has no Distributed front end, so its records and watermark are per-shard"
    );
    let local = statements
        .iter()
        .find(|s| s.contains("CREATE TABLE IF NOT EXISTS otel_spans_local"))
        .expect("the local span table is generated");
    let distributed = statements
        .iter()
        .find(|s| s.contains("CREATE TABLE IF NOT EXISTS otel_spans ON CLUSTER"))
        .expect("the distributed span table is generated");

    // Local tables should use ReplicatedReplacingMergeTree
    assert!(local.contains("ReplicatedReplacingMergeTree"));
    assert!(local.contains("ON CLUSTER"));

    // Distributed tables should use Distributed engine
    assert!(distributed.contains("ENGINE = Distributed"));
}

#[test]
fn test_get_insert_table_single_node() {
    let config = default_config();
    assert_eq!(get_insert_table(&config, "otel_spans"), "otel_spans");
}

#[test]
fn test_get_insert_table_distributed() {
    let config = ClickhouseConfig {
        cluster: Some("test_cluster".to_string()),
        distributed: true,
        ..default_config()
    };
    // The distributed front end, not `_local`: it is what applies the sharding key, and a write
    // aimed past it lands on whichever node the connection reached.
    assert_eq!(get_insert_table(&config, "otel_spans"), "otel_spans");
}

#[test]
fn tenant_policies_are_fail_closed_and_target_physical_tables() {
    let single = tenant_row_policies(&default_config());
    assert_eq!(single.len(), 7);
    assert!(single.iter().all(|policy| {
        policy.contains("getSettingOrDefault('SQL_sideseat_project_id', '')")
            && policy.contains("getSettingOrDefault('SQL_sideseat_maintenance', 0) = 1")
            && policy.contains(" ON `sideseat`.")
            && !policy.contains("_local")
    }));

    let distributed = tenant_row_policies(&ClickhouseConfig {
        cluster: Some("test_cluster".to_owned()),
        distributed: true,
        ..default_config()
    });
    assert!(distributed.iter().all(|policy| {
        policy.contains(" ON `sideseat`.")
            && policy.contains("_local ON CLUSTER test_cluster")
            && !policy.contains(" ON `sideseat`.otel_spans ON CLUSTER")
            && !policy.contains(" ON `sideseat`.otel_metrics ON CLUSTER")
            && !policy.contains(" ON `sideseat`.otel_logs ON CLUSTER")
    }));
}

#[test]
fn tenant_policy_database_is_quoted_as_one_identifier() {
    let policies = tenant_row_policies(&ClickhouseConfig {
        database: "tenant-db`blue".to_owned(),
        ..default_config()
    });
    assert!(
        policies
            .iter()
            .all(|policy| policy.contains(r"ON `tenant-db\`blue`."))
    );
}

/// The single-node span table, found by name.
///
/// These three tests indexed `statements[1]`, which was the span table only as long as nothing was
/// inserted before it. Adding `span_partition_anomalies` shifted every one of them onto a table with no
/// `LowCardinality`, no TTL and no bloom filters - so all three would have started asserting against the
/// wrong statement, and a positional index is why. Naming the table is the fix.
fn single_node_spans_schema() -> String {
    generate_schema(&default_config())
        .into_iter()
        .find(|s| s.contains("CREATE TABLE IF NOT EXISTS otel_spans"))
        .expect("the span table is generated")
}

#[test]
fn test_schema_has_low_cardinality() {
    let spans_schema = single_node_spans_schema();
    assert!(spans_schema.contains("LowCardinality(String)"));
    assert!(spans_schema.contains("LowCardinality(Nullable(String))"));
}

#[test]
fn test_schema_has_ttl() {
    let schemas = generate_schema(&default_config());
    for (table, timestamp) in [
        ("otel_spans", "timestamp_start"),
        ("otel_metrics", "timestamp"),
        ("otel_logs", "timestamp"),
    ] {
        let schema = schemas
            .iter()
            .find(|statement| statement.contains(&format!("CREATE TABLE IF NOT EXISTS {table} ")))
            .unwrap_or_else(|| panic!("{table} schema"));
        assert!(schema.contains(&format!(
            "TTL greatest({timestamp} + INTERVAL 90 DAY, coalesce(hold_until, \
                 toDateTime64(0, 6, 'UTC'))) DELETE"
        )));
        assert!(
            !schema.contains(&format!("TTL {timestamp} + INTERVAL 90 DAY DELETE")),
            "{table} still has an unconditional TTL"
        );
    }
}

#[test]
fn test_schema_has_indices() {
    let spans_schema = single_node_spans_schema();
    assert!(spans_schema.contains("INDEX idx_trace_id"));
    assert!(spans_schema.contains("INDEX idx_session_id"));
    assert!(spans_schema.contains("INDEX idx_span_id"));
    assert!(spans_schema.contains("bloom_filter"));
}

#[test]
fn test_get_delete_table_single_node() {
    let config = default_config();
    assert_eq!(get_delete_table(&config, "otel_spans"), "otel_spans");
    assert_eq!(get_delete_table(&config, "otel_metrics"), "otel_metrics");
}

#[test]
fn test_get_delete_table_distributed() {
    let config = ClickhouseConfig {
        cluster: Some("test_cluster".to_string()),
        distributed: true,
        ..default_config()
    };
    assert_eq!(get_delete_table(&config, "otel_spans"), "otel_spans_local");
    assert_eq!(
        get_delete_table(&config, "otel_metrics"),
        "otel_metrics_local"
    );
}

/// A version bump without a migration is a database that cannot start.
///
/// `apply_versioned_migration` walks `(current+1)..=SCHEMA_VERSION` and fails on any version it
/// does not know, so bumping the constant without adding an entry turns every existing database
/// into a startup error. That failure belongs here, not at a user's first restart after upgrading.
#[test]
fn test_get_on_cluster_clause_single_node() {
    let config = default_config();
    assert_eq!(get_on_cluster_clause(&config), "");
}

#[test]
fn test_get_on_cluster_clause_distributed() {
    let config = ClickhouseConfig {
        cluster: Some("test_cluster".to_string()),
        distributed: true,
        ..default_config()
    };
    assert_eq!(get_on_cluster_clause(&config), " ON CLUSTER test_cluster");
}

#[test]
fn test_get_on_cluster_clause_distributed_default_cluster() {
    let config = ClickhouseConfig {
        cluster: None,
        distributed: true,
        ..default_config()
    };
    assert_eq!(get_on_cluster_clause(&config), " ON CLUSTER default");
}
