//! Credential repository for PostgreSQL operations

use sqlx::PgPool;

use crate::data::postgres::PostgresError;
use crate::data::types::CredentialRow;

/// List all credentials for an organization
pub async fn list_credentials(
    pool: &PgPool,
    org_id: &str,
) -> Result<Vec<CredentialRow>, PostgresError> {
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
           FROM credentials WHERE organization_id = $1 ORDER BY created_at ASC, id ASC"#,
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
    pool: &PgPool,
    id: &str,
    org_id: &str,
) -> Result<Option<CredentialRow>, PostgresError> {
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
           FROM credentials WHERE id = $1 AND organization_id = $2"#,
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
    pool: &PgPool,
    id: &str,
    org_id: &str,
    provider_key: &str,
    display_name: &str,
    endpoint_url: Option<&str>,
    extra_config: Option<&str>,
    key_preview: Option<&str>,
    created_by: Option<&str>,
) -> Result<CredentialRow, PostgresError> {
    let now = chrono::Utc::now().timestamp();

    sqlx::query(
        r#"INSERT INTO credentials
           (id, organization_id, provider_key, display_name, endpoint_url, extra_config,
            key_preview, created_by, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"#,
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
    pool: &PgPool,
    id: &str,
    org_id: &str,
    display_name: Option<&str>,
    endpoint_url: Option<Option<&str>>,
    extra_config: Option<Option<&str>>,
) -> Result<Option<CredentialRow>, PostgresError> {
    let now = chrono::Utc::now().timestamp();

    // Build dynamic SET clause
    let mut set_parts: Vec<String> = vec!["updated_at = $1".to_string()];
    let mut param_idx = 2usize;

    if display_name.is_some() {
        set_parts.push(format!("display_name = ${}", param_idx));
        param_idx += 1;
    }
    if endpoint_url.is_some() {
        set_parts.push(format!("endpoint_url = ${}", param_idx));
        param_idx += 1;
    }
    if extra_config.is_some() {
        set_parts.push(format!("extra_config = ${}", param_idx));
        param_idx += 1;
    }

    let sql = format!(
        "UPDATE credentials SET {} WHERE id = ${} AND organization_id = ${}",
        set_parts.join(", "),
        param_idx,
        param_idx + 1,
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
    pool: &PgPool,
    id: &str,
    org_id: &str,
) -> Result<bool, PostgresError> {
    let result = sqlx::query("DELETE FROM credentials WHERE id = $1 AND organization_id = $2")
        .bind(id)
        .bind(org_id)
        .execute(pool)
        .await?;

    Ok(result.rows_affected() > 0)
}
