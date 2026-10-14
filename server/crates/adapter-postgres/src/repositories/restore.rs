//! Queries used only while reconciling independently restored stores.

use sqlx::PgConnection;

use crate::PostgresError;

pub async fn association_trace_ids(
    connection: &mut PgConnection,
    project_id: &str,
    after_trace_id: Option<&str>,
    limit: usize,
) -> Result<Vec<String>, PostgresError> {
    let rows = sqlx::query_scalar(
        "SELECT trace_id
           FROM trace_files
          WHERE project_id = $1
            AND ($2::text IS NULL OR trace_id > $2)
          ORDER BY trace_id
          LIMIT $3",
    )
    .bind(project_id)
    .bind(after_trace_id)
    .bind(i64::try_from(limit).unwrap_or(i64::MAX))
    .fetch_all(&mut *connection)
    .await?;
    Ok(rows)
}

/// Move `files`' id sequence past `above`, every row's id and its own last value, never back.
///
/// `setval` with the default `is_called` makes the next id the value plus one. A sequence never used has no last
/// value; the floor of 1 is its minimum. One value can be skipped, which costs nothing: incarnations need only be
/// unique and increasing.
pub async fn advance_file_incarnations(
    connection: &mut PgConnection,
    above: i64,
) -> Result<(), PostgresError> {
    sqlx::query(
        "SELECT setval(
                    pg_get_serial_sequence('files', 'id'),
                    GREATEST(
                        $1,
                        (SELECT COALESCE(MAX(id), 0) FROM files),
                        COALESCE(pg_sequence_last_value(pg_get_serial_sequence('files', 'id')::regclass), 0),
                        1
                    )
                )",
    )
    .bind(above)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// Clear a file's deletion claim: see `FileMetaStore::restore_clear_deletion_claim`.
pub async fn clear_deletion_claim(
    connection: &mut PgConnection,
    project_id: &str,
    file_hash: &str,
) -> Result<(), PostgresError> {
    sqlx::query(
        "UPDATE files SET deleting_at = NULL
          WHERE project_id = $1 AND file_hash = $2 AND deleting_at IS NOT NULL",
    )
    .bind(project_id)
    .bind(file_hash)
    .execute(&mut *connection)
    .await?;
    Ok(())
}
