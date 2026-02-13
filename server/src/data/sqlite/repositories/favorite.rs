//! Favorite repository for SQLite operations

use std::collections::HashSet;

use sqlx::SqlitePool;

use crate::data::sqlite::SqliteError;

/// Add a favorite for a user (idempotent)
/// Returns true if created, false if already existed
pub async fn add_favorite(
    pool: &SqlitePool,
    user_id: &str,
    project_id: &str,
    entity_type: &str,
    entity_id: &str,
    secondary_id: Option<&str>,
) -> Result<bool, SqliteError> {
    let now = chrono::Utc::now().timestamp();

    let result = sqlx::query(
        r#"
        INSERT INTO favorites (user_id, project_id, entity_type, entity_id, secondary_id, created_at)
        VALUES (?, ?, ?, ?, ?, ?)
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
    pool: &SqlitePool,
    user_id: &str,
    project_id: &str,
    entity_type: &str,
    entity_id: &str,
    secondary_id: Option<&str>,
) -> Result<bool, SqliteError> {
    let result = if secondary_id.is_some() {
        sqlx::query(
            r#"
            DELETE FROM favorites
            WHERE user_id = ? AND project_id = ? AND entity_type = ?
              AND entity_id = ? AND secondary_id = ?
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
            WHERE user_id = ? AND project_id = ? AND entity_type = ?
              AND entity_id = ? AND secondary_id IS NULL
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
    pool: &SqlitePool,
    user_id: &str,
    project_id: &str,
    entity_type: &str,
    limit: u32,
) -> Result<Vec<String>, SqliteError> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        r#"
        SELECT entity_id, secondary_id
        FROM favorites
        WHERE user_id = ? AND project_id = ? AND entity_type = ?
        ORDER BY created_at DESC
        LIMIT ?
        "#,
    )
    .bind(user_id)
    .bind(project_id)
    .bind(entity_type)
    .bind(limit)
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
    pool: &SqlitePool,
    user_id: &str,
    project_id: &str,
    entity_type: &str,
    ids: &[String],
) -> Result<HashSet<String>, SqliteError> {
    if ids.is_empty() {
        return Ok(HashSet::new());
    }

    // Build placeholders for IN clause
    let placeholders: String = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let query = format!(
        r#"
        SELECT entity_id
        FROM favorites
        WHERE user_id = ? AND project_id = ? AND entity_type = ?
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
    pool: &SqlitePool,
    user_id: &str,
    project_id: &str,
    spans: &[(String, String)], // (trace_id, span_id) pairs
) -> Result<HashSet<String>, SqliteError> {
    if spans.is_empty() {
        return Ok(HashSet::new());
    }

    // Build OR conditions for each span
    let conditions: String = spans
        .iter()
        .map(|_| "(entity_id = ? AND secondary_id = ?)")
        .collect::<Vec<_>>()
        .join(" OR ");

    let query = format!(
        r#"
        SELECT entity_id, secondary_id
        FROM favorites
        WHERE user_id = ? AND project_id = ? AND entity_type = 'span'
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
    pool: &SqlitePool,
    user_id: &str,
    project_id: &str,
) -> Result<u64, SqliteError> {
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM favorites WHERE user_id = ? AND project_id = ?")
            .bind(user_id)
            .bind(project_id)
            .fetch_one(pool)
            .await?;

    Ok(count as u64)
}

/// Delete favorites by entity IDs (for cleanup after trace/session deletion)
/// Returns the number of rows deleted
pub async fn delete_favorites_by_entity(
    pool: &SqlitePool,
    project_id: &str,
    entity_type: &str,
    entity_ids: &[String],
) -> Result<u64, SqliteError> {
    if entity_ids.is_empty() {
        return Ok(0);
    }

    // Build placeholders for IN clause
    let placeholders: String = entity_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let query = format!(
        r#"
        DELETE FROM favorites
        WHERE project_id = ? AND entity_type = ? AND entity_id IN ({})
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_test_pool() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::query(crate::data::sqlite::schema::SCHEMA)
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    #[tokio::test]
    async fn test_add_favorite() {
        let pool = setup_test_pool().await;

        // First add should return true (created)
        let created = add_favorite(&pool, "local", "default", "trace", "trace-123", None)
            .await
            .unwrap();
        assert!(created);

        // Second add should return false (already existed)
        let created = add_favorite(&pool, "local", "default", "trace", "trace-123", None)
            .await
            .unwrap();
        assert!(!created);
    }

    #[tokio::test]
    async fn test_add_span_favorite() {
        let pool = setup_test_pool().await;

        // Add span favorite with secondary_id
        let created = add_favorite(
            &pool,
            "local",
            "default",
            "span",
            "trace-123",
            Some("span-456"),
        )
        .await
        .unwrap();
        assert!(created);

        // Duplicate should return false
        let created = add_favorite(
            &pool,
            "local",
            "default",
            "span",
            "trace-123",
            Some("span-456"),
        )
        .await
        .unwrap();
        assert!(!created);

        // Same trace, different span should work
        let created = add_favorite(
            &pool,
            "local",
            "default",
            "span",
            "trace-123",
            Some("span-789"),
        )
        .await
        .unwrap();
        assert!(created);
    }

    #[tokio::test]
    async fn test_remove_favorite() {
        let pool = setup_test_pool().await;

        // Add then remove
        add_favorite(&pool, "local", "default", "trace", "trace-123", None)
            .await
            .unwrap();

        remove_favorite(&pool, "local", "default", "trace", "trace-123", None)
            .await
            .unwrap();

        // Verify removed
        let favorites = check_favorites(
            &pool,
            "local",
            "default",
            "trace",
            &["trace-123".to_string()],
        )
        .await
        .unwrap();
        assert!(favorites.is_empty());
    }

    #[tokio::test]
    async fn test_remove_favorite_idempotent() {
        let pool = setup_test_pool().await;

        // Remove non-existent should not error
        remove_favorite(&pool, "local", "default", "trace", "nonexistent", None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_check_favorites() {
        let pool = setup_test_pool().await;

        add_favorite(&pool, "local", "default", "trace", "trace-1", None)
            .await
            .unwrap();
        add_favorite(&pool, "local", "default", "trace", "trace-3", None)
            .await
            .unwrap();

        let favorites = check_favorites(
            &pool,
            "local",
            "default",
            "trace",
            &[
                "trace-1".to_string(),
                "trace-2".to_string(),
                "trace-3".to_string(),
            ],
        )
        .await
        .unwrap();

        assert_eq!(favorites.len(), 2);
        assert!(favorites.contains("trace-1"));
        assert!(favorites.contains("trace-3"));
        assert!(!favorites.contains("trace-2"));
    }

    #[tokio::test]
    async fn test_check_favorites_empty() {
        let pool = setup_test_pool().await;

        let favorites = check_favorites(&pool, "local", "default", "trace", &[])
            .await
            .unwrap();
        assert!(favorites.is_empty());
    }

    #[tokio::test]
    async fn test_check_span_favorites() {
        let pool = setup_test_pool().await;

        add_favorite(&pool, "local", "default", "span", "trace-1", Some("span-a"))
            .await
            .unwrap();
        add_favorite(&pool, "local", "default", "span", "trace-1", Some("span-b"))
            .await
            .unwrap();

        let favorites = check_span_favorites(
            &pool,
            "local",
            "default",
            &[
                ("trace-1".to_string(), "span-a".to_string()),
                ("trace-1".to_string(), "span-c".to_string()),
            ],
        )
        .await
        .unwrap();

        assert_eq!(favorites.len(), 1);
        assert!(favorites.contains("trace-1:span-a"));
        assert!(!favorites.contains("trace-1:span-c"));
    }

    #[tokio::test]
    async fn test_list_all_favorite_ids() {
        let pool = setup_test_pool().await;

        add_favorite(&pool, "local", "default", "trace", "trace-1", None)
            .await
            .unwrap();
        add_favorite(&pool, "local", "default", "trace", "trace-2", None)
            .await
            .unwrap();
        add_favorite(&pool, "local", "default", "session", "session-1", None)
            .await
            .unwrap();

        let trace_ids = list_all_favorite_ids(&pool, "local", "default", "trace", 100)
            .await
            .unwrap();
        assert_eq!(trace_ids.len(), 2);

        let session_ids = list_all_favorite_ids(&pool, "local", "default", "session", 100)
            .await
            .unwrap();
        assert_eq!(session_ids.len(), 1);
    }

    #[tokio::test]
    async fn test_list_all_favorite_ids_with_limit() {
        let pool = setup_test_pool().await;

        for i in 0..10 {
            add_favorite(
                &pool,
                "local",
                "default",
                "trace",
                &format!("trace-{}", i),
                None,
            )
            .await
            .unwrap();
        }

        let ids = list_all_favorite_ids(&pool, "local", "default", "trace", 5)
            .await
            .unwrap();
        assert_eq!(ids.len(), 5);
    }

    #[tokio::test]
    async fn test_count_favorites() {
        let pool = setup_test_pool().await;

        let count = count_favorites(&pool, "local", "default").await.unwrap();
        assert_eq!(count, 0);

        add_favorite(&pool, "local", "default", "trace", "trace-1", None)
            .await
            .unwrap();
        add_favorite(&pool, "local", "default", "session", "session-1", None)
            .await
            .unwrap();

        let count = count_favorites(&pool, "local", "default").await.unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_delete_favorites_by_entity() {
        let pool = setup_test_pool().await;

        add_favorite(&pool, "local", "default", "trace", "trace-1", None)
            .await
            .unwrap();
        add_favorite(&pool, "local", "default", "trace", "trace-2", None)
            .await
            .unwrap();
        add_favorite(&pool, "local", "default", "trace", "trace-3", None)
            .await
            .unwrap();

        let deleted = delete_favorites_by_entity(
            &pool,
            "default",
            "trace",
            &["trace-1".to_string(), "trace-2".to_string()],
        )
        .await
        .unwrap();
        assert_eq!(deleted, 2);

        let remaining = list_all_favorite_ids(&pool, "local", "default", "trace", 100)
            .await
            .unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0], "trace-3");
    }

    #[tokio::test]
    async fn test_delete_favorites_by_entity_empty() {
        let pool = setup_test_pool().await;

        let deleted = delete_favorites_by_entity(&pool, "default", "trace", &[])
            .await
            .unwrap();
        assert_eq!(deleted, 0);
    }
}
