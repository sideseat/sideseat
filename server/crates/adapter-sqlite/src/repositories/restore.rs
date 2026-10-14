//! Queries used only while reconciling independently restored stores.

use sqlx::SqlitePool;

use crate::SqliteError;

pub async fn association_trace_ids(
    pool: &SqlitePool,
    project_id: &str,
    after_trace_id: Option<&str>,
    limit: usize,
) -> Result<Vec<String>, SqliteError> {
    let rows = sqlx::query_scalar(
        "SELECT trace_id
           FROM trace_files
          WHERE project_id = ?
            AND (? IS NULL OR trace_id > ?)
          ORDER BY trace_id
          LIMIT ?",
    )
    .bind(project_id)
    .bind(after_trace_id)
    .bind(after_trace_id)
    .bind(i64::try_from(limit).unwrap_or(i64::MAX))
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Move `files`' `AUTOINCREMENT` sequence past `above` and every row's id, never back.
///
/// SQLite gives a new row an id above both `sqlite_sequence`'s value and the table's largest id, so raising the
/// value raises every later incarnation. The sequence has no row for `files` until its first insert.
pub async fn advance_file_incarnations(pool: &SqlitePool, above: i64) -> Result<(), SqliteError> {
    let mut transaction = pool.begin().await?;
    let raised = sqlx::query(
        "UPDATE sqlite_sequence
            SET seq = MAX(seq, ?, (SELECT COALESCE(MAX(id), 0) FROM files))
          WHERE name = 'files'",
    )
    .bind(above)
    .execute(&mut *transaction)
    .await?;
    if raised.rows_affected() == 0 {
        sqlx::query(
            "INSERT INTO sqlite_sequence (name, seq)
             SELECT 'files', MAX(?, (SELECT COALESCE(MAX(id), 0) FROM files))",
        )
        .bind(above)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

/// Clear a file's deletion claim: see `FileMetaStore::restore_clear_deletion_claim`.
pub async fn clear_deletion_claim(
    pool: &SqlitePool,
    project_id: &str,
    file_hash: &str,
) -> Result<(), SqliteError> {
    sqlx::query(
        "UPDATE files SET deleting_at = NULL
          WHERE project_id = ? AND file_hash = ? AND deleting_at IS NOT NULL",
    )
    .bind(project_id)
    .bind(file_hash)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn schema_pool() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.expect("pool");
        sqlx::raw_sql(crate::schema::SCHEMA)
            .execute(&pool)
            .await
            .expect("schema");
        pool
    }

    async fn insert_file(pool: &SqlitePool, hash: &str) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO files (project_id, file_hash, size_bytes, hash_algo, ref_count, created_at, updated_at)
             VALUES ('p', ?, 1, 'sha256', 0, 0, 0) RETURNING id",
        )
        .bind(hash)
        .fetch_one(pool)
        .await
        .expect("file row")
    }

    /// The incarnations issued after a backup are issued again once the backup is restored - here, the sequence
    /// as it was before any row - unless the repair moves the sequence past the incarnations the blob store holds.
    #[tokio::test]
    async fn the_next_incarnation_is_above_the_advance_and_every_row() {
        let pool = schema_pool().await;
        advance_file_incarnations(&pool, 41)
            .await
            .expect("advance with no row yet");
        assert_eq!(insert_file(&pool, "a").await, 42);

        advance_file_incarnations(&pool, 7)
            .await
            .expect("an advance below");
        assert_eq!(insert_file(&pool, "b").await, 43, "never moves back");

        sqlx::query("UPDATE sqlite_sequence SET seq = 0 WHERE name = 'files'")
            .execute(&pool)
            .await
            .expect("a sequence behind its rows");
        advance_file_incarnations(&pool, 0).await.expect("advance");
        let seq: i64 = sqlx::query_scalar("SELECT seq FROM sqlite_sequence WHERE name = 'files'")
            .fetch_one(&pool)
            .await
            .expect("sequence");
        assert_eq!(seq, 43, "past every row");

        advance_file_incarnations(&pool, 100)
            .await
            .expect("advance");
        assert_eq!(insert_file(&pool, "c").await, 101);
    }

    #[tokio::test]
    async fn pages_file_ownership_in_trace_order() {
        let pool = SqlitePool::connect("sqlite::memory:").await.expect("pool");
        sqlx::query("CREATE TABLE trace_files (project_id TEXT NOT NULL, trace_id TEXT NOT NULL);")
            .execute(&pool)
            .await
            .expect("schema");
        for (table, project, trace) in [
            ("trace_files", "p", "a"),
            ("trace_files", "p", "b"),
            ("trace_files", "p", "c"),
            ("trace_files", "other", "z"),
        ] {
            sqlx::query(&format!(
                "INSERT INTO {table} (project_id, trace_id) VALUES (?, ?)"
            ))
            .bind(project)
            .bind(trace)
            .execute(&pool)
            .await
            .expect("association");
        }

        assert_eq!(
            association_trace_ids(&pool, "p", None, 2)
                .await
                .expect("first page"),
            ["a", "b"]
        );
        assert_eq!(
            association_trace_ids(&pool, "p", Some("b"), 2)
                .await
                .expect("second page"),
            ["c"]
        );
    }
}
