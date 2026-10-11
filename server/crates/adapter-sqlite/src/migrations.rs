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
        SchemaCheck::Current => check_layout(pool).await,
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

/// Refuse a store at the current version whose layout is not the one [`SCHEMA`] creates.
///
/// The version says which build created a store, not that its tables match this build's: the layout changes
/// within a version too - the raw arbiter's tables, for one - and a store created before such a change would
/// be read and written as if it had them, failing at the first statement that needs one. So the store's
/// tables, columns and indexes are compared with a fresh schema's, built in memory, and a mismatch is refused
/// with the reset command named.
async fn check_layout(pool: &SqlitePool) -> Result<(), SqliteError> {
    let expected = {
        let fresh = SqlitePool::connect(":memory:").await?;
        sqlx::raw_sql(SCHEMA).execute(&fresh).await?;
        layout(&fresh).await?
    };
    let found = layout(pool).await?;
    let differences: Vec<String> = expected
        .difference(&found)
        .map(|item| format!("missing {item}"))
        .chain(
            found
                .difference(&expected)
                .map(|item| format!("unexpected {item}")),
        )
        .take(5)
        .collect();
    if differences.is_empty() {
        return Ok(());
    }
    Err(SqliteError::LayoutMismatch(
        sideseat_core::schema_version::LayoutMismatch {
            version: SCHEMA_VERSION,
            differences,
        },
    ))
}

/// Every table's and index's definition as SQLite stored it - which carries what column metadata cannot:
/// `CHECK` and `UNIQUE` constraints, `AUTOINCREMENT`, `WITHOUT ROWID` - and every column's type, position,
/// nullability and default, and the names of the indexes SQLite creates for constraints. Sequence and
/// statistics *contents* are excluded: they are data, not layout.
async fn layout(pool: &SqlitePool) -> Result<std::collections::BTreeSet<String>, SqliteError> {
    let mut items = std::collections::BTreeSet::new();
    let definitions: Vec<String> = sqlx::query_scalar(
        "SELECT type || ' ' || name || ' ' || coalesce(sql, '') FROM sqlite_master \
         WHERE type IN ('table', 'index') AND name NOT IN ('sqlite_sequence') AND name NOT LIKE 'sqlite_stat%'",
    )
    .fetch_all(pool)
    .await?;
    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT 'column ' || m.name || '.' || c.name || ' ' || c.type || ' at ' || c.cid \
         || ' notnull ' || c.\"notnull\" || ' default ' || coalesce(c.dflt_value, '') || ' pk ' || c.pk \
         FROM sqlite_master m JOIN pragma_table_info(m.name) c \
         WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite_%'",
    )
    .fetch_all(pool)
    .await?;
    items.extend(definitions);
    items.extend(columns);
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store at this version created before the raw arbiter's tables existed is refused with the reset
    /// instruction rather than written to: the version alone does not prove the layout.
    /// A dropped constraint changes no column's metadata, so the definitions must be compared too: here the
    /// collection state loses the check that keeps a pending decision above its record's high-water mark.
    #[tokio::test]
    async fn a_store_whose_table_lost_a_constraint_is_refused() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::raw_sql(SCHEMA).execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO schema_version (id, version, applied_at, description) VALUES (1, ?, 0, 'schema')",
        )
        .bind(SCHEMA_VERSION)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::raw_sql(
            "DROP TABLE raw_collections;
             CREATE TABLE raw_collections (
                 project_id TEXT NOT NULL, raw_id TEXT NOT NULL, high_water INTEGER NOT NULL DEFAULT -1,
                 pending_epoch INTEGER, decided_at INTEGER, PRIMARY KEY (project_id, raw_id));
             CREATE INDEX IF NOT EXISTS idx_raw_collections_pending
                 ON raw_collections(decided_at) WHERE pending_epoch IS NOT NULL;",
        )
        .execute(&pool)
        .await
        .unwrap();
        let error = ensure_schema(&pool, &crate::TestClock)
            .await
            .expect_err("a table that lost a constraint is another layout");
        assert!(error.to_string().contains("raw_collections"), "{error}");
    }

    #[tokio::test]
    async fn a_store_missing_the_raw_arbiter_tables_is_refused_with_the_reset_named() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::raw_sql(SCHEMA).execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO schema_version (id, version, applied_at, description) VALUES (1, ?, 0, 'schema')",
        )
        .bind(SCHEMA_VERSION)
        .execute(&pool)
        .await
        .unwrap();
        for table in [
            "raw_owners",
            "raw_writers",
            "raw_arbiter",
            "raw_collections",
        ] {
            sqlx::query(&format!("DROP TABLE {table}"))
                .execute(&pool)
                .await
                .unwrap();
        }
        let error = ensure_schema(&pool, &crate::TestClock)
            .await
            .expect_err("an earlier layout of this version must be refused");
        let message = error.to_string();
        assert!(
            message.contains("missing"),
            "names what is missing: {message}"
        );
        assert!(
            message.contains("raw_"),
            "names the arbiter tables: {message}"
        );
        assert!(
            message.contains("sideseat system prune"),
            "names the reset command: {message}"
        );
    }

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
