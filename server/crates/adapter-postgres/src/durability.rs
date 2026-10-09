//! The commit settings an acknowledgement rests on, checked when the server connects.
//!
//! SideSeat answers an OTLP export, and retires a staged payload, once PostgreSQL has committed what it stands for.
//! A commit is only durable when the server flushes its WAL before returning: with `fsync = off` nothing is ever
//! flushed, and with `synchronous_commit = off` a commit returns before its WAL is - so a crash loses
//! transactions the server already reported committed, and with them exports that were answered 200. Either is
//! refused at startup, with the setting named, rather than accepted as a performance trade; ClickHouse's
//! fire-and-forget inserts are refused for the same reason. `local`, `on`, `remote_write` and `remote_apply` all
//! flush the commit locally before returning, and the stronger ones also wait for standbys.
//!
//! Checked on the connections SideSeat writes through, after their role is set, so a default set on the
//! database or on the login role is what is judged - not only the server's.

use sqlx::PgPool;

use crate::PostgresError;

/// Why `fsync` and `synchronous_commit`, as the server reports them, break the acknowledgement boundary - or
/// `None` when they hold it.
pub(crate) fn refusal(fsync: &str, synchronous_commit: &str) -> Option<String> {
    if !fsync.eq_ignore_ascii_case("on") {
        return Some(format!(
            "PostgreSQL runs with fsync = {fsync}: commits are never flushed, so a crash can lose exports SideSeat \
             has already acknowledged. Set fsync = on."
        ));
    }
    if synchronous_commit.eq_ignore_ascii_case("off") {
        return Some(
            "PostgreSQL runs with synchronous_commit = off for SideSeat's connections: a commit returns before \
             its WAL is flushed, so a crash can lose exports SideSeat has already acknowledged. Set it to on \
             (or local, remote_write or remote_apply) for the server, the database and SideSeat's login role."
                .to_string(),
        );
    }
    None
}

/// Refuse a pool whose commits would return before they are durable.
pub(crate) async fn check(pool: &PgPool) -> Result<(), PostgresError> {
    let (fsync, synchronous_commit): (String, String) =
        sqlx::query_as("SELECT current_setting('fsync'), current_setting('synchronous_commit')")
            .fetch_one(pool)
            .await?;
    match refusal(&fsync, &synchronous_commit) {
        Some(reason) => Err(PostgresError::Config(reason)),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_settings_that_flush_every_commit_are_accepted() {
        for level in ["on", "local", "remote_write", "remote_apply", "ON"] {
            assert_eq!(refusal("on", level), None, "{level}");
        }
        let off = refusal("on", "off").expect("asynchronous commit is refused");
        assert!(off.contains("synchronous_commit = off"), "{off}");
        let unsynced = refusal("off", "on").expect("fsync off is refused");
        assert!(unsynced.contains("fsync = off"), "{unsynced}");
    }
}
