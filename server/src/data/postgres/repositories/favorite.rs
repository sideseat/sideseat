//! Favorite repository for PostgreSQL operations

use std::collections::HashSet;

use sqlx::PgPool;

use crate::data::postgres::PostgresError;

/// Add a favorite for a user (idempotent)
/// Returns true if created, false if already existed
pub async fn add_favorite(
    pool: &PgPool,
    user_id: &str,
    project_id: &str,
    entity_type: &str,
    entity_id: &str,
    secondary_id: Option<&str>,
) -> Result<bool, PostgresError> {
    let now = chrono::Utc::now().timestamp();

    let result = sqlx::query(
        r#"
        INSERT INTO favorites (user_id, project_id, entity_type, entity_id, secondary_id, created_at)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(user_id)
    .bind(project_id)
    .bind(entity_type)
    .bind(entity_id)
    .bind(secondary_id)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Remove a favorite (idempotent)
/// Returns true if removed, false if didn't exist
pub async fn remove_favorite(
    pool: &PgPool,
    user_id: &str,
    project_id: &str,
    entity_type: &str,
    entity_id: &str,
    secondary_id: Option<&str>,
) -> Result<bool, PostgresError> {
    let result = if secondary_id.is_some() {
        sqlx::query(
            r#"
            DELETE FROM favorites
            WHERE user_id = $1 AND project_id = $2 AND entity_type = $3
              AND entity_id = $4 AND secondary_id = $5
            "#,
        )
        .bind(user_id)
        .bind(project_id)
        .bind(entity_type)
        .bind(entity_id)
        .bind(secondary_id)
        .execute(pool)
        .await?
    } else {
        sqlx::query(
            r#"
            DELETE FROM favorites
            WHERE user_id = $1 AND project_id = $2 AND entity_type = $3
              AND entity_id = $4 AND secondary_id IS NULL
            "#,
        )
        .bind(user_id)
        .bind(project_id)
        .bind(entity_type)
        .bind(entity_id)
        .execute(pool)
        .await?
    };

    Ok(result.rows_affected() > 0)
}

/// List all favorite entity IDs for a user (for "favorites only" filter)
/// For spans, returns composite "entity_id:secondary_id" strings
/// For other types, returns just entity_id
pub async fn list_all_favorite_ids(
    pool: &PgPool,
    user_id: &str,
    project_id: &str,
    entity_type: &str,
    limit: u32,
) -> Result<Vec<String>, PostgresError> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        r#"
        SELECT entity_id, secondary_id
        FROM favorites
        WHERE user_id = $1 AND project_id = $2 AND entity_type = $3
        ORDER BY created_at DESC
        LIMIT $4
        "#,
    )
    .bind(user_id)
    .bind(project_id)
    .bind(entity_type)
    .bind(limit as i64)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(entity_id, secondary_id)| match secondary_id {
            Some(sid) => format!("{}:{}", entity_id, sid),
            None => entity_id,
        })
        .collect())
}

/// Check which entity IDs are favorited (batch operation)
/// Returns the subset of `ids` that are favorited
pub async fn check_favorites(
    pool: &PgPool,
    user_id: &str,
    project_id: &str,
    entity_type: &str,
    ids: &[String],
) -> Result<HashSet<String>, PostgresError> {
    if ids.is_empty() {
        return Ok(HashSet::new());
    }

    // Build placeholders for IN clause with numbered parameters
    let placeholders: String = ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("${}", i + 4))
        .collect::<Vec<_>>()
        .join(",");
    let query = format!(
        r#"
        SELECT entity_id
        FROM favorites
        WHERE user_id = $1 AND project_id = $2 AND entity_type = $3
          AND entity_id IN ({})
        "#,
        placeholders
    );

    let mut query_builder = sqlx::query_as::<_, (String,)>(&query)
        .bind(user_id)
        .bind(project_id)
        .bind(entity_type);

    for id in ids {
        query_builder = query_builder.bind(id);
    }

    let rows: Vec<(String,)> = query_builder.fetch_all(pool).await?;

    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Check which spans are favorited (batch operation for composite keys)
/// Returns the subset of spans that are favorited as "trace_id:span_id" strings
pub async fn check_span_favorites(
    pool: &PgPool,
    user_id: &str,
    project_id: &str,
    spans: &[(String, String)], // (trace_id, span_id) pairs
) -> Result<HashSet<String>, PostgresError> {
    if spans.is_empty() {
        return Ok(HashSet::new());
    }

    // Build OR conditions for each span with numbered parameters
    let conditions: String = spans
        .iter()
        .enumerate()
        .map(|(i, _)| {
            format!(
                "(entity_id = ${} AND secondary_id = ${})",
                i * 2 + 3,
                i * 2 + 4
            )
        })
        .collect::<Vec<_>>()
        .join(" OR ");

    let query = format!(
        r#"
        SELECT entity_id, secondary_id
        FROM favorites
        WHERE user_id = $1 AND project_id = $2 AND entity_type = 'span'
          AND ({})
        "#,
        conditions
    );

    let mut query_builder = sqlx::query_as::<_, (String, Option<String>)>(&query)
        .bind(user_id)
        .bind(project_id);

    for (trace_id, span_id) in spans {
        query_builder = query_builder.bind(trace_id).bind(span_id);
    }

    let rows: Vec<(String, Option<String>)> = query_builder.fetch_all(pool).await?;

    // Return as "trace_id:span_id" composite keys
    Ok(rows
        .into_iter()
        .filter_map(|(entity_id, secondary_id)| {
            secondary_id.map(|sid| format!("{}:{}", entity_id, sid))
        })
        .collect())
}

/// Count total favorites for a user in a project (for soft limit check)
pub async fn count_favorites(
    pool: &PgPool,
    user_id: &str,
    project_id: &str,
) -> Result<u64, PostgresError> {
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM favorites WHERE user_id = $1 AND project_id = $2")
            .bind(user_id)
            .bind(project_id)
            .fetch_one(pool)
            .await?;

    Ok(count as u64)
}

/// Delete favorites by entity IDs (for cleanup after trace/session deletion)
/// Returns the number of rows deleted
pub async fn delete_favorites_by_entity(
    pool: &PgPool,
    project_id: &str,
    entity_type: &str,
    entity_ids: &[String],
) -> Result<u64, PostgresError> {
    if entity_ids.is_empty() {
        return Ok(0);
    }

    // Build placeholders for IN clause with numbered parameters
    let placeholders: String = entity_ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("${}", i + 3))
        .collect::<Vec<_>>()
        .join(",");
    let query = format!(
        r#"
        DELETE FROM favorites
        WHERE project_id = $1 AND entity_type = $2 AND entity_id IN ({})
        "#,
        placeholders
    );

    let mut query_builder = sqlx::query(&query).bind(project_id).bind(entity_type);

    for id in entity_ids {
        query_builder = query_builder.bind(id);
    }

    let result = query_builder.execute(pool).await?;

    Ok(result.rows_affected())
}
