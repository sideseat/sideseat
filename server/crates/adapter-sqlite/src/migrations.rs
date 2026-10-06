//! Schema creation: the current schema, created in one step; a store at any other version is refused.
//!
//! See `sideseat_core::schema_version`: there are no upgrades, so there are no migrations.

use sqlx::SqlitePool;

use super::error::SqliteError;
use super::schema::{SCHEMA, SCHEMA_VERSION};
use sideseat_core::schema_version::{SchemaCheck, check};
use sideseat_ports::clock::Clock;

/// Create the schema in an empty database, accept the current one, refuse any other.
pub async fn ensure_schema(pool: &SqlitePool, clock: &dyn Clock) -> Result<(), SqliteError> {
    let table_exists: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'schema_version'",
    )
    .fetch_one(pool)
    .await?;
    let found: Option<i32> = if table_exists {
        sqlx::query_scalar("SELECT version FROM schema_version WHERE id = 1")
            .fetch_optional(pool)
            .await?
    } else {
        None
    };
    match check(found, SCHEMA_VERSION).map_err(SqliteError::UnsupportedSchema)? {
        SchemaCheck::Current => Ok(()),
        SchemaCheck::Create => {
            let mut tx = pool.begin().await?;
            // One script, so comments and literals keep ordinary SQL parsing.
            sqlx::raw_sql(SCHEMA).execute(&mut *tx).await?;
            sqlx::query(
                "INSERT INTO schema_version (id, version, applied_at, description) VALUES (1, ?, ?, 'schema')",
            )
            .bind(SCHEMA_VERSION)
            .bind(clock.now().timestamp_nanos_opt().unwrap_or(0))
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_empty_database_gets_the_schema_and_another_version_is_refused() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        ensure_schema(&pool, &crate::TestClock).await.unwrap();
        ensure_schema(&pool, &crate::TestClock).await.unwrap();
        sqlx::query("UPDATE schema_version SET version = 1")
            .execute(&pool)
            .await
            .unwrap();
        let error = ensure_schema(&pool, &crate::TestClock).await.unwrap_err();
        assert!(matches!(
            error,
            SqliteError::UnsupportedSchema(refusal) if refusal.found == 1 && refusal.supported == SCHEMA_VERSION
        ));
    }
}
