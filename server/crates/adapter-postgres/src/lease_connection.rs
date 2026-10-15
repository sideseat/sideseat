//! The connection process-lease statements run on, owned outright rather than pooled.
//!
//! A lease renewal is abandoned by its caller at the loss deadline. A pooled connection dropped mid-statement goes
//! back to its pool through a readiness check that waits on the server without bound, keeping the pool's slot the
//! whole time - so with a one-connection pool a silent peer kept every later renewal waiting for a slot, and the
//! server's fence closed, though a new connection would have worked. Here an attempt takes the connection out,
//! connecting first if there is none, and puts it back only when its statement answered: an abandoned attempt
//! drops it, which closes the socket and asks nothing of the peer, and the next attempt connects again.

use std::time::Duration;

use sqlx::postgres::{PgConnectOptions, PgConnection};
use sqlx::{ConnectOptions, Executor};
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Mutex;

use crate::PostgresError;

/// A lease statement's future, borrowing the connection it runs on.
pub type LeaseWork<'c, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, PostgresError>> + Send + 'c>>;

/// One owned connection for lease statements, connected on demand within `bound`.
pub struct LeaseConnection {
    options: PgConnectOptions,
    role: &'static str,
    bound: Duration,
    slot: Mutex<Slot>,
    /// Set first by [`Self::close`], before it waits for the slot: a call in flight then drops its connection
    /// rather than keeping it, and no later call opens one.
    closed: AtomicBool,
}

/// The connection's state: none open yet, one open, or closed for good.
enum Slot {
    Empty,
    Open(PgConnection),
    Closed,
}

fn timed_out() -> PostgresError {
    PostgresError::Database(sqlx::Error::PoolTimedOut)
}

impl LeaseConnection {
    pub(crate) fn new(options: PgConnectOptions, role: &'static str, bound: Duration) -> Self {
        Self {
            options,
            role,
            bound,
            slot: Mutex::new(Slot::Empty),
            closed: AtomicBool::new(false),
        }
    }

    /// Run `work` on the lease connection. Every client-side wait is bounded by `bound`: for the connection, for
    /// opening it - connect and `SET ROLE` together - and for the statement's answer, which the server's own
    /// `statement_timeout` cannot bound when the peer stops answering. After [`Self::close`] it refuses.
    pub async fn run<T>(
        &self,
        work: impl for<'c> FnOnce(&'c mut PgConnection) -> LeaseWork<'c, T>,
    ) -> Result<T, PostgresError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(PostgresError::Database(sqlx::Error::PoolClosed));
        }
        let mut slot = tokio::time::timeout(self.bound, self.slot.lock())
            .await
            .map_err(|_| timed_out())?;
        if self.closed.load(Ordering::Acquire) {
            *slot = Slot::Closed;
            return Err(PostgresError::Database(sqlx::Error::PoolClosed));
        }
        let mut connection = match std::mem::replace(&mut *slot, Slot::Empty) {
            Slot::Open(connection) => connection,
            Slot::Closed => {
                *slot = Slot::Closed;
                return Err(PostgresError::Database(sqlx::Error::PoolClosed));
            }
            Slot::Empty => tokio::time::timeout(self.bound, async {
                let mut connection = self.options.connect().await?;
                connection
                    .execute(format!("SET ROLE {}", self.role).as_str())
                    .await?;
                Ok::<_, PostgresError>(connection)
            })
            .await
            .map_err(|_| timed_out())??,
        };
        let result = tokio::time::timeout(self.bound, work(&mut connection))
            .await
            .map_err(|_| timed_out())
            .and_then(|answered| answered);
        // Kept only after an answer: a statement that failed, or was not answered, may have left the session in
        // any state, and dropping the connection closes its socket without asking anything of the peer.
        if result.is_ok() && !self.closed.load(Ordering::Acquire) {
            *slot = Slot::Open(connection);
        }
        result
    }

    /// Close the connection for good: later work is refused rather than reconnecting.
    pub(crate) async fn close(&self) {
        self.closed.store(true, Ordering::Release);
        let Ok(mut slot) = tokio::time::timeout(self.bound, self.slot.lock()).await else {
            return;
        };
        if let Slot::Open(connection) = std::mem::replace(&mut *slot, Slot::Closed) {
            let _ = tokio::time::timeout(self.bound, sqlx::Connection::close(connection)).await;
        }
    }
}
