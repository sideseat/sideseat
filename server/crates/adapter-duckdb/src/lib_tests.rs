use super::*;
use tempfile::TempDir;

async fn create_test_storage() -> (TempDir, AppStorage) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let duckdb_dir = temp_dir.path().join("duckdb");
    tokio::fs::create_dir_all(&duckdb_dir)
        .await
        .expect("Failed to create duckdb dir");
    let storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
    (temp_dir, storage)
}

#[tokio::test]
async fn test_analytics_service_init() {
    let (_temp_dir, storage) = create_test_storage().await;
    let result = DuckdbService::init(&storage, std::sync::Arc::new(crate::TestClock)).await;
    assert!(
        result.is_ok(),
        "DuckdbService should initialize successfully"
    );
}

/// The engine's memory limit is the one this crate declares, not 80% of the host's RAM.
///
/// Asserted rather than assumed, because DuckDB's default sizes itself from the machine: on any ordinary
/// server that is two orders of magnitude above the whole process's footprint ceiling, so a ceiling with
/// this `SET` missing or silently ignored would be a statement about everything except the component most
/// likely to breach it. Reads the setting back through `current_setting`, since a `SET` DuckDB accepted and
/// interpreted differently is indistinguishable from one that worked.
/// A query runs on at most [`DUCKDB_MAX_THREADS`] threads, and on fewer when the host has fewer cores: each
/// thread scanning the long text columns holds its own decompressed segments, so the memory limit holds only
/// with the threads bounded too.
#[tokio::test]
async fn the_engine_runs_a_query_on_no_more_threads_than_its_memory_allows() {
    let (_temp_dir, storage) = create_test_storage().await;
    let service = DuckdbService::init(&storage, std::sync::Arc::new(crate::TestClock))
        .await
        .expect("Init should succeed");
    let threads: i64 = service
        .conn()
        .query_row("SELECT current_setting('threads')", [], |row| row.get(0))
        .expect("threads");
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    assert_eq!(
        threads as usize,
        cores.min(sideseat_core::constants::DUCKDB_MAX_THREADS)
    );
}

#[tokio::test]
async fn the_engine_takes_the_declared_memory_limit() {
    let (_temp_dir, storage) = create_test_storage().await;
    let service = DuckdbService::init(&storage, std::sync::Arc::new(crate::TestClock))
        .await
        .expect("Init should succeed");

    let (limit, temp_dir): (String, String) = {
        let conn = service.conn();
        conn.query_row(
            "SELECT current_setting('memory_limit'), current_setting('temp_directory')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("both settings are readable")
    };

    // Compared as bytes rather than against a formatted string: DuckDB normalises the value it was given
    // ("200.0 MiB" for 209715200 bytes), and pinning its formatting would make this a test of DuckDB's
    // display code. Within 1 MiB, because that normalisation rounds.
    let reported = parse_duckdb_size(&limit)
        .unwrap_or_else(|| panic!("could not read a byte count out of {limit:?}"));
    let declared = sideseat_core::constants::DUCKDB_MEMORY_LIMIT_BYTES as f64;
    assert!(
        (reported - declared).abs() < 1_048_576.0,
        "the engine reports a {limit} limit, which is not the declared {declared} bytes"
    );

    assert!(
        !temp_dir.is_empty(),
        "a tight memory limit is only safe because DuckDB can spill, so it needs somewhere to spill to"
    );
}

/// Bytes from a DuckDB size string such as `200.0 MiB`, `1.5GB` or `1024`.
fn parse_duckdb_size(value: &str) -> Option<f64> {
    let trimmed = value.trim();
    let split = trimmed
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(split);
    let number: f64 = number.parse().ok()?;
    let scale = match unit.trim().to_ascii_uppercase().as_str() {
        "" | "B" => 1.0,
        "KIB" | "KB" => 1024.0,
        "MIB" | "MB" => 1024.0 * 1024.0,
        "GIB" | "GB" => 1024.0 * 1024.0 * 1024.0,
        "TIB" | "TB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some(number * scale)
}

#[tokio::test]
async fn test_analytics_service_conn() {
    let (_temp_dir, storage) = create_test_storage().await;
    let service = DuckdbService::init(&storage, std::sync::Arc::new(crate::TestClock))
        .await
        .expect("Init should succeed");

    let conn = service.conn();
    drop(conn); // Successfully acquired connection
}

#[tokio::test]
async fn test_analytics_service_checkpoint() {
    let (_temp_dir, storage) = create_test_storage().await;
    let service = Arc::new(
        DuckdbService::init(&storage, std::sync::Arc::new(crate::TestClock))
            .await
            .expect("Init should succeed"),
    );

    let result = service.checkpoint().await;
    assert!(result.is_ok(), "Checkpoint should succeed");
}

#[tokio::test]
async fn test_analytics_service_schema_applied() {
    let (_temp_dir, storage) = create_test_storage().await;
    let service = DuckdbService::init(&storage, std::sync::Arc::new(crate::TestClock))
        .await
        .expect("Init should succeed");

    let conn = service.conn();
    let version: i32 = conn
        .query_row(
            "SELECT version FROM schema_version WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .expect("Should read schema version");

    assert_eq!(version, schema::SCHEMA_VERSION);
}

#[tokio::test]
async fn test_analytics_service_is_open() {
    let (_temp_dir, storage) = create_test_storage().await;
    let service = Arc::new(
        DuckdbService::init(&storage, std::sync::Arc::new(crate::TestClock))
            .await
            .expect("Init should succeed"),
    );

    assert!(service.is_open(), "Connection should be open after init");
}

#[tokio::test]
async fn test_analytics_service_close() {
    let (_temp_dir, storage) = create_test_storage().await;
    let service = Arc::new(
        DuckdbService::init(&storage, std::sync::Arc::new(crate::TestClock))
            .await
            .expect("Init should succeed"),
    );

    assert!(service.is_open());
    let result = service.close().await;
    assert!(result.is_ok(), "Close should succeed");
}

#[tokio::test]
async fn test_checkpoint_after_close_is_noop() {
    let (_temp_dir, storage) = create_test_storage().await;
    let service = Arc::new(
        DuckdbService::init(&storage, std::sync::Arc::new(crate::TestClock))
            .await
            .expect("Init should succeed"),
    );

    // Clone before close since close consumes Arc
    let service_for_checkpoint = Arc::clone(&service);

    service.close().await.expect("Close should succeed");

    // Checkpoint after close should be a no-op, not panic
    let result = service_for_checkpoint.checkpoint().await;
    assert!(
        result.is_ok(),
        "Checkpoint after close should succeed as no-op"
    );
}

/// New files are created in the storage format that honours `USING COMPRESSION zstd`, and long text is
/// compressed with it once checkpointed.
#[tokio::test]
async fn files_are_created_in_the_compressing_storage_format() {
    let (_temp_dir, storage) = create_test_storage().await;
    let service = DuckdbService::init(&storage, std::sync::Arc::new(crate::TestClock))
        .await
        .unwrap();
    let conn = service.conn();
    let version: String = conn
        .query_row(
            "SELECT tags['storage_version'] FROM duckdb_databases() WHERE database_name = current_database()",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, format!("{DUCKDB_STORAGE_VERSION}+"));
    conn.execute_batch(
        "INSERT INTO otel_raw SELECT 'p', 'r' || i, 'traces', TIMESTAMP '2026-01-01', 'received', 1,
             TIMESTAMP '2026-01-01', NULL, encode(repeat('history ', 300) || i) FROM range(3000) t(i);
         CHECKPOINT;",
    )
    .unwrap();
    let compression: String = conn
        .query_row(
            "SELECT string_agg(DISTINCT compression, ',') FROM pragma_storage_info('otel_raw') \
             WHERE column_name = 'record' AND segment_type = 'BLOB'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(compression, "ZSTD");
}
