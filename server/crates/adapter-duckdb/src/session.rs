//! The analytics file's connection: opened in the storage format the schema needs, with the session settings
//! every statement relies on.

use duckdb::Connection;
use sideseat_core::constants::{DUCKDB_MAX_THREADS, DUCKDB_MEMORY_LIMIT_BYTES};

use crate::DuckdbError;

/// Rows an index scan may find before DuckDB reads the whole table instead; see the connection settings.
const INDEX_SCAN_MAX_COUNT: u64 = 1 << 40;

/// The DuckDB storage format analytics files are created in: the first that compresses a column with zstd.
pub(crate) const DUCKDB_STORAGE_VERSION: &str = "v1.5.0";

/// Open the analytics file with the session settings every connection needs.
pub(crate) fn open_configured(
    db_path: &std::path::Path,
    temp_dir: &std::path::Path,
) -> Result<Connection, DuckdbError> {
    // New files in the storage format that honours the schema's `USING COMPRESSION zstd`; DuckDB's default
    // (`v0.10.2`) stores long strings uncompressed.
    let config =
        duckdb::Config::default().with("storage_compatibility_version", DUCKDB_STORAGE_VERSION)?;
    let conn = Connection::open_with_flags(db_path, config)?;
    // Doubled single quotes, because a path is not a literal until it is escaped and a user's data
    // directory may contain an apostrophe. `SET` takes no bind parameters, so this is the escape.
    let temp_dir_literal = temp_dir.display().to_string().replace('\'', "''");
    // An index scan is taken only while the rows it finds stay under `index_scan_max_count` (2048 by default) or
    // a thousandth of the table, and past that DuckDB reads the whole table instead. The keyed reads
    // (`sideseat_query_sql::keyed`) are the only reads whose scans carry an indexed key alone, and they ask for
    // rows by identity: a span corrected several times, a long trace, a span id a client reuses across projects.
    // Their cost must follow what they ask for, so the index is kept however many rows a key finds.
    conn.execute_batch(&format!(
        "SET index_scan_max_count = {INDEX_SCAN_MAX_COUNT};
         SET autoinstall_known_extensions = false;
         SET autoload_known_extensions = false;
         SET extension_directory = '';
         SET force_compression = 'auto';
         SET memory_limit = '{limit}B';
         SET threads = {threads};
         SET temp_directory = '{temp_dir_literal}';
         PRAGMA enable_checkpoint_on_shutdown;
         LOAD json;",
        limit = DUCKDB_MEMORY_LIMIT_BYTES,
        threads = std::thread::available_parallelism()
            .map_or(1, std::num::NonZeroUsize::get)
            .clamp(1, DUCKDB_MAX_THREADS),
    ))?;
    Ok(conn)
}
