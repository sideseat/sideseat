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
           FROM (
                 SELECT trace_id FROM trace_files WHERE project_id = ?
                 UNION
                 SELECT trace_id FROM span_bodies WHERE project_id = ?
                ) AS candidates
          WHERE (? IS NULL OR trace_id > ?)
          ORDER BY trace_id
          LIMIT ?",
    )
    .bind(project_id)
    .bind(project_id)
    .bind(after_trace_id)
    .bind(after_trace_id)
    .bind(i64::try_from(limit).unwrap_or(i64::MAX))
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pages_the_union_of_file_and_body_ownership() {
        let pool = SqlitePool::connect("sqlite::memory:").await.expect("pool");
        sqlx::query(
            "CREATE TABLE trace_files (project_id TEXT NOT NULL, trace_id TEXT NOT NULL);
             CREATE TABLE span_bodies (project_id TEXT NOT NULL, trace_id TEXT NOT NULL);",
        )
        .execute(&pool)
        .await
        .expect("schema");
        for (table, project, trace) in [
            ("trace_files", "p", "a"),
            ("trace_files", "p", "c"),
            ("span_bodies", "p", "b"),
            ("span_bodies", "p", "c"),
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
