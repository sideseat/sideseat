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
           FROM (
                 SELECT trace_id FROM trace_files WHERE project_id = $1
                 UNION
                 SELECT trace_id FROM span_bodies WHERE project_id = $1
                ) AS candidates
          WHERE ($2::text IS NULL OR trace_id > $2)
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
