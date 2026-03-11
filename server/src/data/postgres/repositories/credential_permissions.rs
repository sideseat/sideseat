//! Credential project permissions repository for PostgreSQL operations

use sqlx::PgPool;

use crate::data::postgres::PostgresError;
use crate::data::types::CredentialPermissionRow;

/// List all permissions for a credential
pub async fn list_credential_permissions(
    pool: &PgPool,
    credential_id: &str,
) -> Result<Vec<CredentialPermissionRow>, PostgresError> {
    let rows = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            Option<String>,
            String,
            Option<String>,
            i64,
            i64,
        ),
    >(
        r#"SELECT id, credential_id, organization_id, project_id, access, created_by,
                  created_at, updated_at
           FROM credential_project_permissions
           WHERE credential_id = $1
           ORDER BY created_at ASC, id ASC"#,
    )
    .bind(credential_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(id, credential_id, organization_id, project_id, access, created_by, created_at, updated_at)| {
                CredentialPermissionRow {
                    id,
                    credential_id,
                    organization_id,
                    project_id,
                    access,
                    created_by,
                    created_at,
                    updated_at,
                }
            },
        )
        .collect())
}

/// Create a credential project permission
pub async fn create_credential_permission(
    pool: &PgPool,
    id: &str,
    credential_id: &str,
    org_id: &str,
    project_id: Option<&str>,
    access: &str,
    created_by: Option<&str>,
) -> Result<CredentialPermissionRow, PostgresError> {
    let now = chrono::Utc::now().timestamp();

    sqlx::query(
        r#"INSERT INTO credential_project_permissions
           (id, credential_id, organization_id, project_id, access, created_by, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"#,
    )
    .bind(id)
    .bind(credential_id)
    .bind(org_id)
    .bind(project_id)
    .bind(access)
    .bind(created_by)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(CredentialPermissionRow {
        id: id.to_string(),
        credential_id: credential_id.to_string(),
        organization_id: org_id.to_string(),
        project_id: project_id.map(ToString::to_string),
        access: access.to_string(),
        created_by: created_by.map(ToString::to_string),
        created_at: now,
        updated_at: now,
    })
}

/// Delete a credential permission by id. Returns true if deleted.
pub async fn delete_credential_permission(
    pool: &PgPool,
    id: &str,
    credential_id: &str,
) -> Result<bool, PostgresError> {
    let result = sqlx::query(
        "DELETE FROM credential_project_permissions WHERE id = $1 AND credential_id = $2",
    )
    .bind(id)
    .bind(credential_id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Get credential IDs accessible by a specific project.
pub async fn get_credentials_accessible_by_project(
    pool: &PgPool,
    org_id: &str,
    project_id: &str,
) -> Result<Vec<String>, PostgresError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        r#"SELECT c.id FROM credentials c
           WHERE c.organization_id = $1
           AND NOT EXISTS (
               SELECT 1 FROM credential_project_permissions
               WHERE credential_id = c.id AND project_id = $2 AND access = 'deny'
           )
           AND (
               NOT EXISTS (
                   SELECT 1 FROM credential_project_permissions
                   WHERE credential_id = c.id AND access = 'allow'
               )
               OR EXISTS (
                   SELECT 1 FROM credential_project_permissions
                   WHERE credential_id = c.id AND project_id = $2 AND access = 'allow'
               )
               OR EXISTS (
                   SELECT 1 FROM credential_project_permissions
                   WHERE credential_id = c.id AND project_id IS NULL AND access = 'allow'
               )
           )
           ORDER BY c.created_at ASC, c.id ASC"#,
    )
    .bind(org_id)
    .bind(project_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|(id,)| id).collect())
}
