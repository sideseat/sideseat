//! Credential repository for SQLite operations

use sqlx::SqlitePool;

use crate::data::sqlite::SqliteError;
use crate::data::types::CredentialRow;

/// List all credentials for an organization
pub async fn list_credentials(
    pool: &SqlitePool,
    org_id: &str,
) -> Result<Vec<CredentialRow>, SqliteError> {
    let rows = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
            i64,
        ),
    >(
        r#"SELECT id, organization_id, provider_key, display_name, endpoint_url, extra_config,
                  key_preview, created_by, created_at, updated_at
           FROM credentials WHERE organization_id = ? ORDER BY created_at ASC, id ASC"#,
    )
    .bind(org_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                organization_id,
                provider_key,
                display_name,
                endpoint_url,
                extra_config,
                key_preview,
                created_by,
                created_at,
                updated_at,
            )| CredentialRow {
                id,
                organization_id,
                provider_key,
                display_name,
                endpoint_url,
                extra_config,
                key_preview,
                created_by,
                created_at,
                updated_at,
            },
        )
        .collect())
}

/// Get a single credential by id, scoped to org
pub async fn get_credential(
    pool: &SqlitePool,
    id: &str,
    org_id: &str,
) -> Result<Option<CredentialRow>, SqliteError> {
    let row = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
            i64,
        ),
    >(
        r#"SELECT id, organization_id, provider_key, display_name, endpoint_url, extra_config,
                  key_preview, created_by, created_at, updated_at
           FROM credentials WHERE id = ? AND organization_id = ?"#,
    )
    .bind(id)
    .bind(org_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(
            id,
            organization_id,
            provider_key,
            display_name,
            endpoint_url,
            extra_config,
            key_preview,
            created_by,
            created_at,
            updated_at,
        )| CredentialRow {
            id,
            organization_id,
            provider_key,
            display_name,
            endpoint_url,
            extra_config,
            key_preview,
            created_by,
            created_at,
            updated_at,
        },
    ))
}

/// Create a new credential row
#[allow(clippy::too_many_arguments)]
pub async fn create_credential(
    pool: &SqlitePool,
    id: &str,
    org_id: &str,
    provider_key: &str,
    display_name: &str,
    endpoint_url: Option<&str>,
    extra_config: Option<&str>,
    key_preview: Option<&str>,
    created_by: Option<&str>,
) -> Result<CredentialRow, SqliteError> {
    let now = chrono::Utc::now().timestamp();

    sqlx::query(
        r#"INSERT INTO credentials
           (id, organization_id, provider_key, display_name, endpoint_url, extra_config,
            key_preview, created_by, created_at, updated_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(id)
    .bind(org_id)
    .bind(provider_key)
    .bind(display_name)
    .bind(endpoint_url)
    .bind(extra_config)
    .bind(key_preview)
    .bind(created_by)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(CredentialRow {
        id: id.to_string(),
        organization_id: org_id.to_string(),
        provider_key: provider_key.to_string(),
        display_name: display_name.to_string(),
        endpoint_url: endpoint_url.map(ToString::to_string),
        extra_config: extra_config.map(ToString::to_string),
        key_preview: key_preview.map(ToString::to_string),
        created_by: created_by.map(ToString::to_string),
        created_at: now,
        updated_at: now,
    })
}

/// Update credential metadata. Returns updated row or None if not found.
/// `endpoint_url`/`extra_config`: None = don't change, Some(None) = clear, Some(Some(v)) = set.
pub async fn update_credential(
    pool: &SqlitePool,
    id: &str,
    org_id: &str,
    display_name: Option<&str>,
    endpoint_url: Option<Option<&str>>,
    extra_config: Option<Option<&str>>,
) -> Result<Option<CredentialRow>, SqliteError> {
    let now = chrono::Utc::now().timestamp();

    // Build dynamic SET clause — only include fields that are Some
    let mut set_parts: Vec<&str> = vec!["updated_at = ?"];

    if display_name.is_some() {
        set_parts.push("display_name = ?");
    }
    if endpoint_url.is_some() {
        set_parts.push("endpoint_url = ?");
    }
    if extra_config.is_some() {
        set_parts.push("extra_config = ?");
    }

    let sql = format!(
        "UPDATE credentials SET {} WHERE id = ? AND organization_id = ?",
        set_parts.join(", ")
    );

    let mut q = sqlx::query(&sql).bind(now);

    if let Some(v) = display_name {
        q = q.bind(v);
    }
    if let Some(v) = endpoint_url {
        q = q.bind(v);
    }
    if let Some(v) = extra_config {
        q = q.bind(v);
    }

    let result = q.bind(id).bind(org_id).execute(pool).await?;

    if result.rows_affected() == 0 {
        return Ok(None);
    }

    get_credential(pool, id, org_id).await
}

/// Delete a credential by id, scoped to org. Returns true if deleted.
pub async fn delete_credential(
    pool: &SqlitePool,
    id: &str,
    org_id: &str,
) -> Result<bool, SqliteError> {
    let result = sqlx::query("DELETE FROM credentials WHERE id = ? AND organization_id = ?")
        .bind(id)
        .bind(org_id)
        .execute(pool)
        .await?;

    Ok(result.rows_affected() > 0)
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
    async fn test_create_and_list() {
        let pool = setup_test_pool().await;
        let id = cuid2::create_id();

        let row = create_credential(
            &pool,
            &id,
            "default",
            "anthropic",
            "Anthropic Key",
            None,
            None,
            Some("sk-ant-ap"),
            Some("local"),
        )
        .await
        .unwrap();

        assert_eq!(row.id, id);
        assert_eq!(row.provider_key, "anthropic");
        assert_eq!(row.display_name, "Anthropic Key");
        assert_eq!(row.key_preview, Some("sk-ant-ap".to_string()));

        let list = list_credentials(&pool, "default").await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, id);
    }

    #[tokio::test]
    async fn test_get_credential() {
        let pool = setup_test_pool().await;
        let id = cuid2::create_id();

        create_credential(
            &pool, &id, "default", "openai", "OpenAI", None, None, None, None,
        )
        .await
        .unwrap();

        let found = get_credential(&pool, &id, "default").await.unwrap();
        assert!(found.is_some());

        // Wrong org
        let not_found = get_credential(&pool, &id, "other-org").await.unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn test_update_credential() {
        let pool = setup_test_pool().await;
        let id = cuid2::create_id();

        create_credential(
            &pool,
            &id,
            "default",
            "ollama",
            "Ollama Local",
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let updated = update_credential(
            &pool,
            &id,
            "default",
            Some("Ollama Updated"),
            Some(Some("http://localhost:11434")),
            None,
        )
        .await
        .unwrap();

        assert!(updated.is_some());
        let u = updated.unwrap();
        assert_eq!(u.display_name, "Ollama Updated");
        assert_eq!(u.endpoint_url, Some("http://localhost:11434".to_string()));
    }

    #[tokio::test]
    async fn test_update_credential_clear_field() {
        let pool = setup_test_pool().await;
        let id = cuid2::create_id();

        create_credential(
            &pool,
            &id,
            "default",
            "azure-ai-foundry",
            "Azure Key",
            Some("https://my.openai.azure.com/"),
            Some(r#"{"api_version":"2024-02-01"}"#),
            None,
            None,
        )
        .await
        .unwrap();

        // Clear endpoint_url via Some(None)
        let updated = update_credential(&pool, &id, "default", None, Some(None), None)
            .await
            .unwrap()
            .unwrap();

        assert!(
            updated.endpoint_url.is_none(),
            "endpoint_url should be cleared"
        );
        // extra_config is untouched
        assert!(
            updated.extra_config.is_some(),
            "extra_config should be unchanged"
        );
    }

    #[tokio::test]
    async fn test_delete_credential() {
        let pool = setup_test_pool().await;
        let id = cuid2::create_id();

        create_credential(
            &pool, &id, "default", "groq", "Groq Key", None, None, None, None,
        )
        .await
        .unwrap();

        let deleted = delete_credential(&pool, &id, "default").await.unwrap();
        assert!(deleted);

        let not_found = get_credential(&pool, &id, "default").await.unwrap();
        assert!(not_found.is_none());
    }
}
