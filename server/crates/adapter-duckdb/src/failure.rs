//! The database's fatal failure, once it has had one.
//!
//! DuckDB invalidates a database instance when it raises a fatal error - a checkpoint that cannot complete, the
//! auto-checkpoint after a commit among them - and every statement afterwards fails with "database has been
//! invalidated", until the process opens the database again. Nothing on this connection can recover it, so the
//! service records the first such error here, health reports it, and the server exits on it; the next process
//! replays the write-ahead log, which holds every commit DuckDB reported durable.
//!
//! The driver reports a statement's failure as a state code and a message only, so the error is recognised by
//! what the database does next. A statement's message names a fatal error ("FATAL Error: ..."), but an
//! appender's carries no such prefix, and an internal error that invalidated the instance is not itself fatal -
//! so every other database error is followed by one probe statement on the connection, which an invalidated
//! instance refuses with a fatal error. Errors are rare, and a probe is microseconds; the success path pays
//! nothing. A failed statement is settled into [`DuckdbError::Invalidated`] once, so the probe is not repeated
//! on its way out.

use std::sync::Arc;

use duckdb::Connection;
use tokio::sync::watch;

use crate::DuckdbError;

/// DuckDB's spelling of a fatal exception's message. An invalidated instance's refusal is one, and carries the
/// error that invalidated it.
const FATAL_PREFIX: &str = "FATAL Error:";

impl DuckdbError {
    /// The message, when this error invalidated the database or met it invalidated.
    pub fn fatal_message(&self) -> Option<&str> {
        match self {
            Self::Invalidated(message) => Some(message),
            Self::Database(duckdb::Error::DuckDBFailure(_, Some(message)))
                if message.starts_with(FATAL_PREFIX) =>
            {
                Some(message)
            }
            _ => None,
        }
    }
}

/// The refusal of an invalidated database to run a statement on `conn`, if it is invalidated.
fn refusal(conn: &Connection) -> Option<String> {
    let error = conn.execute_batch("SELECT 1").err()?;
    let message = error.to_string();
    message.starts_with(FATAL_PREFIX).then_some(message)
}

/// The first fatal error the database raised, kept for the life of the service and published to whoever waits
/// on it.
pub(crate) struct FatalFailure(watch::Sender<Option<Arc<str>>>);

impl FatalFailure {
    pub(crate) fn new() -> Self {
        Self(watch::channel(None).0)
    }

    /// `error`, as [`DuckdbError::Invalidated`] - and recorded - if it is fatal or left the database invalidated.
    ///
    /// `conn` is the connection the failed work ran on, still held, or `None` when it has been closed; a probe
    /// needs it. Held, so the failure is recorded before another statement can meet the database invalidated.
    /// Any error is probed, not only the database's: work can fail on its own and then meet a fatal error it
    /// only logs, as a rollback does.
    pub(crate) fn settle(&self, conn: Option<&Connection>, error: DuckdbError) -> DuckdbError {
        if matches!(error, DuckdbError::Invalidated(_)) {
            return error;
        }
        let message = match error.fatal_message() {
            Some(message) => message.to_string(),
            // The refusal names the error that invalidated the database, which need not be this one.
            None => match conn.and_then(refusal) {
                Some(refusal) => format!("{refusal} (met by: {error})"),
                None => return error,
            },
        };
        self.record(&message);
        DuckdbError::Invalidated(message)
    }

    fn record(&self, message: &str) {
        let recorded = self.0.send_if_modified(|failure| {
            failure
                .is_none()
                .then(|| *failure = Some(Arc::from(message)))
                .is_some()
        });
        if recorded {
            tracing::error!(
                error = message,
                "DuckDB invalidated the analytics database after a fatal error; the server must restart to \
                 open it again"
            );
        }
    }

    pub(crate) fn get(&self) -> Option<Arc<str>> {
        self.0.borrow().clone()
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<Option<Arc<str>>> {
        self.0.subscribe()
    }
}
