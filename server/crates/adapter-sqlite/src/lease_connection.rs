//! The connection process-lease statements run on, owned outright rather than pooled.
//!
//! The PostgreSQL twin explains why: a pooled connection dropped mid-statement keeps its pool's slot while its
//! return is checked. Here an attempt takes the connection out, opening it first if there is none, and puts it back
//! only when its statement answered; an abandoned attempt drops it. A store built from a pool - an in-memory database,
//! which a second connection would not see - shares that pool instead.

use std::time::Duration;

use sqlx::ConnectOptions;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection, SqlitePool};
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Mutex;

use crate::SqliteError;

/// A lease statement's future, borrowing the connection it runs on.
pub type LeaseWork<'c, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, SqliteError>> + Send + 'c>>;

/// The lease statements' connection: owned and opened on demand within `bound`, or a shared pool.
pub struct LeaseConnection {
    source: Source,
    bound: Duration,
    /// Set first by [`Self::close`], before it waits for the slot: a call in flight then drops its connection
    /// rather than keeping it, and no later call opens one.
    closed: AtomicBool,
}

enum Source {
    Owned {
        options: Box<SqliteConnectOptions>,
        slot: Mutex<Slot>,
    },
    Shared(SqlitePool),
}

/// The owned connection's state: none open yet, one open, or closed for good.
pub(crate) enum Slot {
    Empty,
    Open(SqliteConnection),
    Closed,
}

impl LeaseConnection {
    pub(crate) fn owned(options: SqliteConnectOptions, bound: Duration) -> Self {
        Self {
            source: Source::Owned {
                options: Box::new(options),
                slot: Mutex::new(Slot::Empty),
            },
            bound,
            closed: AtomicBool::new(false),
        }
    }

    pub(crate) fn shared(pool: SqlitePool, bound: Duration) -> Self {
        Self {
            source: Source::Shared(pool),
            bound,
            closed: AtomicBool::new(false),
        }
    }

    /// Run `work` on the lease connection. Every wait is bounded by `bound`: for the connection, for opening it,
    /// and for the statement's answer (its lock waits also by the busy timeout). After [`Self::close`] it refuses.
    pub async fn run<T>(
        &self,
        work: impl for<'c> FnOnce(&'c mut SqliteConnection) -> LeaseWork<'c, T>,
    ) -> Result<T, SqliteError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(SqliteError::Database(sqlx::Error::PoolClosed));
        }
        let timed_out = || SqliteError::Database(sqlx::Error::PoolTimedOut);
        match &self.source {
            Source::Shared(pool) => {
                let mut connection = tokio::time::timeout(self.bound, pool.acquire())
                    .await
                    .map_err(|_| timed_out())??;
                tokio::time::timeout(self.bound, work(&mut connection))
                    .await
                    .map_err(|_| timed_out())?
            }
            Source::Owned { options, slot } => {
                let mut slot = tokio::time::timeout(self.bound, slot.lock())
                    .await
                    .map_err(|_| timed_out())?;
                if self.closed.load(Ordering::Acquire) {
                    *slot = Slot::Closed;
                    return Err(SqliteError::Database(sqlx::Error::PoolClosed));
                }
                let mut connection = match std::mem::replace(&mut *slot, Slot::Empty) {
                    Slot::Open(connection) => connection,
                    Slot::Closed => {
                        *slot = Slot::Closed;
                        return Err(SqliteError::Database(sqlx::Error::PoolClosed));
                    }
                    Slot::Empty => tokio::time::timeout(self.bound, options.connect())
                        .await
                        .map_err(|_| timed_out())??,
                };
                let result = tokio::time::timeout(self.bound, work(&mut connection))
                    .await
                    .map_err(|_| timed_out())
                    .and_then(|answered| answered);
                // Kept only after an answer: a failed or unanswered statement may have left the connection
                // mid-transaction.
                if result.is_ok() && !self.closed.load(Ordering::Acquire) {
                    *slot = Slot::Open(connection);
                }
                result
            }
        }
    }

    /// Close the owned connection for good: later work is refused rather than reopening it.
    pub(crate) async fn close(&self) {
        self.closed.store(true, Ordering::Release);
        let Source::Owned { slot, .. } = &self.source else {
            return;
        };
        let Ok(mut slot) = tokio::time::timeout(self.bound, slot.lock()).await else {
            return;
        };
        if let Slot::Open(connection) = std::mem::replace(&mut *slot, Slot::Closed) {
            let _ = tokio::time::timeout(self.bound, sqlx::Connection::close(connection)).await;
        }
    }

    /// The owned connection's slot, for tests that hold it.
    #[cfg(test)]
    pub(crate) fn slot_for_test(&self) -> &Mutex<Slot> {
        match &self.source {
            Source::Owned { slot, .. } => slot,
            Source::Shared(_) => panic!("a shared lease connection has no slot"),
        }
    }
}
