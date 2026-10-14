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
        SchemaCheck::Current => check_layout(conn).await,
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

/// Refuse a store at the current version whose layout is not the one [`SCHEMA`] creates.
///
/// The version says which build created a store, not that its tables match this build's: the layout changes
/// within a version too - the raw arbiter's tables, for one - and a store created before such a change would
/// be read and written as if it had them, failing at the first statement that needs one. The expected layout
/// is built in a scratch schema inside a transaction that is rolled back, so nothing persists, and compared
/// with the store's; a mismatch is refused with the reset command named.
async fn check_layout(conn: &mut PgConnection) -> Result<(), PostgresError> {
    let mut tx = conn.begin().await?;
    sqlx::raw_sql(
        "CREATE SCHEMA sideseat_layout_check; SET LOCAL search_path TO sideseat_layout_check",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::raw_sql(SCHEMA).execute(&mut *tx).await?;
    let expected = layout(&mut tx, "sideseat_layout_check").await?;
    let found = layout(&mut tx, "public").await?;
    tx.rollback().await?;
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
    Err(PostgresError::LayoutMismatch(
        sideseat_core::schema_version::LayoutMismatch {
            version: SCHEMA_VERSION,
            differences,
        },
    ))
}

/// Every column with its type, position, nullability and default, and every index with its definition, with
/// schema qualifiers stripped so the scratch and live layouts compare equal.
async fn layout(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    schema: &str,
) -> Result<std::collections::BTreeSet<String>, PostgresError> {
    let mut items = std::collections::BTreeSet::new();
    let columns: Vec<String> = sqlx::query_scalar(
        // Sequence defaults qualify their schema in one namespace and not the other
        // (`nextval('public.x_seq')` against `nextval('x_seq')`), so the qualifier is stripped before
        // comparing.
        "SELECT 'column ' || table_name || '.' || column_name || ' ' || data_type \
         || ' at ' || ordinal_position || ' nullable ' || is_nullable \
         || ' default ' || replace(coalesce(column_default, ''), table_schema || '.', '') \
         || ' identity ' || is_identity \
         FROM information_schema.columns WHERE table_schema = $1",
    )
    .bind(schema)
    .fetch_all(&mut **tx)
    .await?;
    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT 'index ' || indexname || ' ' || replace(indexdef, schemaname || '.', '') \
         FROM pg_indexes WHERE schemaname = $1",
    )
    .bind(schema)
    .fetch_all(&mut **tx)
    .await?;
    // Constraints too - `CHECK`, `UNIQUE`, primary and foreign keys - which column metadata does not carry.
    let constraints: Vec<String> = sqlx::query_scalar(
        "SELECT 'constraint ' || t.relname || '.' || c.conname || ' ' \
         || replace(pg_get_constraintdef(c.oid), n.nspname || '.', '') \
         FROM pg_constraint c JOIN pg_class t ON t.oid = c.conrelid \
         JOIN pg_namespace n ON n.oid = t.relnamespace WHERE n.nspname = $1",
    )
    .bind(schema)
    .fetch_all(&mut **tx)
    .await?;
    // Sequences a statement draws from by name, which no column default names, so column metadata alone would
    // pass a store that lacks one - and every statement drawing from it would then fail.
    let sequences: Vec<String> = sqlx::query_scalar(
        "SELECT 'sequence ' || sequence_name || ' ' || data_type \
         FROM information_schema.sequences WHERE sequence_schema = $1",
    )
    .bind(schema)
    .fetch_all(&mut **tx)
    .await?;
    items.extend(columns);
    items.extend(indexes);
    items.extend(constraints);
    items.extend(sequences);
    Ok(items)
}
