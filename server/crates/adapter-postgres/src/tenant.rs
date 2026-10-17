//! A transaction scoped to one project, begun and scoped in one round trip.
//!
//! The project selector (`sideseat.project_id`, which every row-level policy reads) is set by the statement that
//! begins the transaction - `BEGIN; SELECT set_config(...)`, one simple-protocol query - rather than by a second
//! statement after it. sqlx's own `begin_with` cannot be used for that: its guard rolls back only once the begin
//! has succeeded, so a begin whose second half fails, or whose future is dropped while it runs, returns the
//! connection to the pool inside an aborted or open transaction - one that still carries the selector. This guard
//! owns the connection from before the begin is sent until its commit has answered, and any other end takes it out
//! of the pool's reuse until it is rolled back: by a task, or closed where the rollback fails or never runs.

use std::ops::{Deref, DerefMut};

use sideseat_ports::types::ProjectId;
use sqlx::pool::PoolConnection;
use sqlx::postgres::PgTransactionManager;
use sqlx::{Executor as _, PgConnection, PgPool, Postgres, TransactionManager as _};

use crate::PostgresError;

/// An open tenant transaction: the connection, until its commit has answered.
pub struct TenantTransaction {
    connection: Option<PoolConnection<Postgres>>,
    /// The runtime the transaction began on, which its rollback runs on whatever thread drops it - a pooled
    /// connection's own drop needs one too.
    runtime: tokio::runtime::Handle,
    /// Whether sqlx's transaction manager counts the transaction as begun - its depth, which nested `begin`s
    /// (savepoints) rest on. Until then a failed or abandoned begin is undone with a plain `ROLLBACK`.
    begun: bool,
}

impl TenantTransaction {
    /// Acquire a connection and begin a transaction on it scoped to `project_id`.
    pub(crate) async fn begin(
        pool: &PgPool,
        project_id: &ProjectId,
    ) -> Result<Self, PostgresError> {
        let begin = begin_statement(project_id)?;
        let mut transaction = Self {
            connection: Some(pool.acquire().await?),
            runtime: tokio::runtime::Handle::current(),
            begun: false,
        };
        // Armed before the begin is sent: a failed or abandoned begin ends like any other unfinished transaction.
        // Begun through sqlx's transaction manager, so the connection's depth counts it and a nested `begin` on it
        // is a savepoint.
        let connection: &mut PgConnection = &mut transaction;
        PgTransactionManager::begin(connection, Some(begin.into())).await?;
        transaction.begun = true;
        Ok(transaction)
    }

    /// Commit, and hand the connection back to the pool. The transaction keeps the connection until the commit has
    /// answered: a commit that fails, or is abandoned while it waits, ends like any unfinished transaction.
    pub async fn commit(mut self) -> Result<(), sqlx::Error> {
        let connection: &mut PgConnection = &mut self;
        PgTransactionManager::commit(connection).await?;
        drop(self.connection.take());
        Ok(())
    }
}

impl Deref for TenantTransaction {
    type Target = PgConnection;

    fn deref(&self) -> &PgConnection {
        self.connection
            .as_deref()
            .expect("an open transaction has its connection")
    }
}

impl DerefMut for TenantTransaction {
    fn deref_mut(&mut self) -> &mut PgConnection {
        self.connection
            .as_deref_mut()
            .expect("an open transaction has its connection")
    }
}

impl Drop for TenantTransaction {
    /// An unfinished transaction is rolled back before its connection is reused - by a task on the transaction's
    /// runtime, since dropping cannot wait. The connection is closed unless that rollback answers: a task that
    /// fails, or that never runs because its runtime is gone, closes it rather than return it to the pool.
    fn drop(&mut self) {
        let Some(connection) = self.connection.take() else {
            return;
        };
        // Inside the transaction's runtime, so the pool's own spawn on dropping a connection has one, whichever thread
        // this is - even where that runtime has shut down and refuses the task.
        let _entered = self.runtime.enter();
        let mut pending = Unreusable(Some(connection));
        let begun = self.begun;
        self.runtime.spawn(async move {
            let rolled_back = match pending.0.as_deref_mut() {
                // Through the manager, so the depth it counted is undone with the transaction.
                Some(raw) if begun => PgTransactionManager::rollback(raw).await.is_ok(),
                Some(raw) => raw.execute("ROLLBACK").await.is_ok(),
                None => false,
            };
            if rolled_back {
                // Back to the pool: it is outside any transaction again.
                drop(pending.0.take());
            }
        });
    }
}

/// A connection that is closed when dropped, unless it is taken out first.
struct Unreusable(Option<PoolConnection<Postgres>>);

impl Drop for Unreusable {
    fn drop(&mut self) {
        if let Some(mut connection) = self.0.take() {
            connection.close_on_drop();
        }
    }
}

/// The statement that begins a tenant transaction and sets its project selector, as one simple-protocol query. The
/// id travels as a literal, so a malformed one is refused rather than quoted: escaping is the second line of
/// defence, not the first.
pub(crate) fn begin_statement(project_id: &ProjectId) -> Result<String, PostgresError> {
    let id = project_id.as_str();
    if !ProjectId::is_well_formed(id) {
        return Err(PostgresError::Config(format!(
            "refusing a malformed project id for a tenant transaction ({} characters)",
            id.chars().count()
        )));
    }
    Ok(format!(
        "BEGIN; SELECT set_config('sideseat.project_id', '{}', true)",
        id.replace('\'', "''")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The selector rides on the statement that begins the transaction, so the id is a literal there: a hostile or
    /// malformed id is refused outright, never quoted into a statement.
    #[test]
    fn only_a_well_formed_project_id_begins_a_tenant_transaction() {
        assert_eq!(
            begin_statement(&ProjectId::new("proj_1-a")).unwrap(),
            "BEGIN; SELECT set_config('sideseat.project_id', 'proj_1-a', true)"
        );
        for hostile in [
            "",
            "a'b",
            "x'); DROP TABLE projects; --",
            "a\\b",
            "a\0b",
            "a b",
            "a;b",
            "a\nb",
            "$$",
            &"a".repeat(65),
        ] {
            assert!(
                begin_statement(&ProjectId::new(hostile)).is_err(),
                "{hostile:?} must be refused"
            );
        }
    }
}
