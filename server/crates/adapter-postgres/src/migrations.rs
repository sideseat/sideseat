//! Schema creation: the current schema, created in one step; a store at any other version is refused.
//!
//! See `sideseat_core::schema_version`: there are no upgrades. Creation holds a session-level advisory lock, so
//! several instances starting against one empty database create it once.

use sqlx::postgres::PgConnection;
use sqlx::{Acquire, PgPool};

use super::error::PostgresError;
use super::schema::{DEFAULT_DATA, SCHEMA, SCHEMA_VERSION, TENANT_RLS_SQL};
use sideseat_core::schema_version::{SchemaCheck, check};
use sideseat_ports::clock::Clock;

/// Create the schema in an empty database, accept the current one, refuse any other.
pub async fn ensure_schema(pool: &PgPool, clock: &dyn Clock) -> Result<(), PostgresError> {
    // "SdSe" in hex: avoids colliding with other applications' advisory locks.
    const SCHEMA_LOCK_ID: i64 = 0x5364_5365;

    let mut conn = pool.acquire().await?;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(SCHEMA_LOCK_ID)
        .execute(&mut *conn)
        .await?;
    let result = ensure_schema_locked(&mut conn, clock).await;
    // Advisory locks are session-scoped, so release through the same connection even after a failure.
    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(SCHEMA_LOCK_ID)
        .execute(&mut *conn)
        .await;
    result
}

async fn ensure_schema_locked(
    conn: &mut PgConnection,
    clock: &dyn Clock,
) -> Result<(), PostgresError> {
    let table_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name = 'schema_version')",
    )
    .fetch_one(&mut *conn)
    .await?;
    let found: Option<i32> = if table_exists {
        sqlx::query_scalar("SELECT version FROM schema_version WHERE id = 1")
            .fetch_optional(&mut *conn)
            .await?
    } else {
        None
    };
    match check(found, SCHEMA_VERSION).map_err(PostgresError::UnsupportedSchema)? {
        SchemaCheck::Current => Ok(()),
        SchemaCheck::Create => {
            let mut tx = conn.begin().await?;
            // Each SQL document as a script, so comments and literals keep ordinary SQL parsing.
            sqlx::raw_sql(SCHEMA).execute(&mut *tx).await?;
            sqlx::raw_sql(TENANT_RLS_SQL).execute(&mut *tx).await?;
            sqlx::raw_sql(DEFAULT_DATA).execute(&mut *tx).await?;
            sqlx::query(
                "INSERT INTO schema_version (id, version, applied_at, description) \
                 VALUES (1, $1, $2, 'schema')",
            )
            .bind(SCHEMA_VERSION)
            .bind(clock.now().timestamp())
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            Ok(())
        }
    }
}
