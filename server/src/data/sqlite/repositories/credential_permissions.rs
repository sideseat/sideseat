//! Credential project permissions repository for SQLite operations

use sqlx::SqlitePool;

use crate::data::sqlite::SqliteError;
use crate::data::types::CredentialPermissionRow;

/// List all permissions for a credential
pub async fn list_credential_permissions(
    pool: &SqlitePool,
    credential_id: &str,
) -> Result<Vec<CredentialPermissionRow>, SqliteError> {
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
           WHERE credential_id = ?
           ORDER BY created_at ASC, id ASC"#,
    )
    .bind(credential_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                credential_id,
                organization_id,
                project_id,
                access,
                created_by,
                created_at,
                updated_at,
            )| {
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
    pool: &SqlitePool,
    id: &str,
    credential_id: &str,
    org_id: &str,
    project_id: Option<&str>,
    access: &str,
    created_by: Option<&str>,
) -> Result<CredentialPermissionRow, SqliteError> {
    let now = chrono::Utc::now().timestamp();

    sqlx::query(
        r#"INSERT INTO credential_project_permissions
           (id, credential_id, organization_id, project_id, access, created_by, created_at, updated_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?)"#,
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
    pool: &SqlitePool,
    id: &str,
    credential_id: &str,
) -> Result<bool, SqliteError> {
    let result = sqlx::query(
        "DELETE FROM credential_project_permissions WHERE id = ? AND credential_id = ?",
    )
    .bind(id)
    .bind(credential_id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Get credential IDs accessible by a specific project.
///
/// Logic:
/// - Exclude credentials with an explicit deny for this project
/// - Include credentials that either:
///   - have no allow rules at all (open by default), or
///   - have an allow rule specifically for this project, or
///   - have an org-level allow rule (project_id IS NULL)
pub async fn get_credentials_accessible_by_project(
    pool: &SqlitePool,
    org_id: &str,
    project_id: &str,
) -> Result<Vec<String>, SqliteError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        r#"SELECT c.id FROM credentials c
           WHERE c.organization_id = ?
           AND NOT EXISTS (
               SELECT 1 FROM credential_project_permissions
               WHERE credential_id = c.id AND project_id = ? AND access = 'deny'
           )
           AND (
               NOT EXISTS (
                   SELECT 1 FROM credential_project_permissions
                   WHERE credential_id = c.id AND access = 'allow'
               )
               OR EXISTS (
                   SELECT 1 FROM credential_project_permissions
                   WHERE credential_id = c.id AND project_id = ? AND access = 'allow'
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
    .bind(project_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|(id,)| id).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::sqlite::repositories::credentials::{create_credential, delete_credential};

    async fn setup_test_pool() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::query(crate::data::sqlite::schema::SCHEMA)
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    #[tokio::test]
    async fn test_create_and_list_permissions() {
        let pool = setup_test_pool().await;
        let cred_id = cuid2::create_id();
        let perm_id = cuid2::create_id();

        create_credential(
            &pool,
            &cred_id,
            "default",
            "anthropic",
            "Key",
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let perm = create_credential_permission(
            &pool,
            &perm_id,
            &cred_id,
            "default",
            Some("default"),
            "allow",
            None,
        )
        .await
        .unwrap();

        assert_eq!(perm.credential_id, cred_id);
        assert_eq!(perm.project_id, Some("default".to_string()));
        assert_eq!(perm.access, "allow");

        let list = list_credential_permissions(&pool, &cred_id).await.unwrap();
        assert_eq!(list.len(), 1);
    }

    #[tokio::test]
    async fn test_accessible_by_project_default_allow() {
        let pool = setup_test_pool().await;
        let cred_id = cuid2::create_id();

        create_credential(
            &pool, &cred_id, "default", "openai", "Key", None, None, None, None,
        )
        .await
        .unwrap();

        // No permissions → accessible by default
        let accessible = get_credentials_accessible_by_project(&pool, "default", "default")
            .await
            .unwrap();
        assert!(accessible.contains(&cred_id));
    }

    #[tokio::test]
    async fn test_accessible_by_project_deny() {
        let pool = setup_test_pool().await;
        let cred_id = cuid2::create_id();
        let perm_id = cuid2::create_id();

        create_credential(
            &pool, &cred_id, "default", "openai", "Key", None, None, None, None,
        )
        .await
        .unwrap();

        create_credential_permission(
            &pool,
            &perm_id,
            &cred_id,
            "default",
            Some("default"),
            "deny",
            None,
        )
        .await
        .unwrap();

        let accessible = get_credentials_accessible_by_project(&pool, "default", "default")
            .await
            .unwrap();
        assert!(!accessible.contains(&cred_id));
    }

    #[tokio::test]
    async fn test_delete_permission() {
        let pool = setup_test_pool().await;
        let cred_id = cuid2::create_id();
        let perm_id = cuid2::create_id();

        create_credential(
            &pool, &cred_id, "default", "openai", "Key", None, None, None, None,
        )
        .await
        .unwrap();

        create_credential_permission(
            &pool,
            &perm_id,
            &cred_id,
            "default",
            Some("default"),
            "allow",
            None,
        )
        .await
        .unwrap();

        let deleted = delete_credential_permission(&pool, &perm_id, &cred_id)
            .await
            .unwrap();
        assert!(deleted);

        let list = list_credential_permissions(&pool, &cred_id).await.unwrap();
        assert!(list.is_empty());

        // Test cascade delete from credential
        create_credential_permission(
            &pool,
            &cuid2::create_id(),
            &cred_id,
            "default",
            None,
            "allow",
            None,
        )
        .await
        .unwrap();

        delete_credential(&pool, &cred_id, "default").await.unwrap();

        let after_cascade = list_credential_permissions(&pool, &cred_id).await.unwrap();
        assert!(after_cascade.is_empty());
    }
}
