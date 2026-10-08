//! ClickHouse error types.

use sideseat_ports::error::DataError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ClickhouseError {
    #[error("Database error: {0}")]
    Database(#[from] clickhouse::error::Error),

    #[error(transparent)]
    UnsupportedSchema(sideseat_core::schema_version::UnsupportedSchema),

    #[error("Connection error: {0}")]
    Connection(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Query timeout after {timeout_secs}s")]
    Timeout { timeout_secs: u64 },
}

/// Server exception codes that answer a write without settling it.
///
/// `TIMEOUT_EXCEEDED` (159) is what an async insert's expired wait and a distributed insert's timeout report,
/// and both leave the rows queued to land later; `SOCKET_TIMEOUT` (209) and `NETWORK_ERROR` (210) are the server
/// losing its own peer mid-write; `UNKNOWN_STATUS_OF_INSERT` (319) is a quorum insert the server itself cannot
/// vouch for; `QUERY_WAS_CANCELLED` (394) stopped the waiter, not necessarily the write.
const WRITE_IN_DOUBT_CODES: [u32; 5] = [159, 209, 210, 319, 394];

impl ClickhouseError {
    /// Whether a failed write may have been applied, or may still be, where a read made now cannot see it.
    ///
    /// Only the server's own answer settles a write: an exception it returned means it ran the insert to its end,
    /// so a read made afterwards sees whatever it stored. Everything else is in doubt - a lost connection, a
    /// client-side timeout, an error raised while the body was streaming (some blocks can already have
    /// committed), a proxy's status page in place of the server's answer, and the answers listed in
    /// [`WRITE_IN_DOUBT_CODES`]. In doubt is the default because the two mistakes are not alike: one keeps a file
    /// association longer than needed, the other releases one under rows that land a moment later.
    pub(crate) fn write_in_doubt(&self) -> bool {
        match self {
            Self::Database(clickhouse::error::Error::BadResponse(answer)) => {
                server_exception_code(answer)
                    .is_none_or(|code| WRITE_IN_DOUBT_CODES.contains(&code))
            }
            _ => true,
        }
    }
}

/// The exception code of a server's answer: `Code: <n>. DB::Exception: ...` in the body, or the bare code the
/// driver falls back to when the body cannot be read. `None` for anything else, such as a proxy's status line.
fn server_exception_code(answer: &str) -> Option<u32> {
    let answer = answer.trim();
    let code = answer
        .strip_prefix("Code: ")
        .and_then(|rest| rest.split_once('.'))
        .map_or(answer, |(code, _)| code);
    code.parse().ok()
}

/// This adapter's error, as the port's error.
///
/// The conversion belongs to the adapter: an adapter knows the port it implements, while the port must not
/// depend on concrete implementations.
impl From<ClickhouseError> for DataError {
    fn from(e: ClickhouseError) -> Self {
        match e {
            // The transience verdict is made **here**, where the driver's error is still in hand. It is a
            // string search, which is a guess - the driver does not classify - but it is a guess about
            // ClickHouse, made in the ClickHouse adapter, rather than one the port makes about a driver it
            // should not know.
            ClickhouseError::Database(e) => {
                let message = e.to_string();
                let transient = message.contains("connection")
                    || message.contains("timeout")
                    || message.contains("network");
                Self::Clickhouse {
                    message,
                    transient,
                    source: Some(Box::new(e)),
                }
            }
            ClickhouseError::UnsupportedSchema(refusal) => {
                Self::unsupported_schema("clickhouse", refusal)
            }
            ClickhouseError::Connection(msg) => Self::Config(msg),
            ClickhouseError::Io(e) => Self::Io(e),
            ClickhouseError::Timeout { timeout_secs } => Self::Timeout {
                backend: "clickhouse",
                timeout_secs,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answered(body: &str) -> ClickhouseError {
        ClickhouseError::Database(clickhouse::error::Error::BadResponse(body.to_string()))
    }

    /// An insert that failed opening - the schema read, before any row was sent - is settled whatever the error:
    /// nothing of it can land.
    #[test]
    fn a_failure_before_any_row_is_sent_is_settled() {
        let unsent = crate::repositories::span::SpanInsertError {
            error: ClickhouseError::Database(clickhouse::error::Error::Network("reset".into())),
            sent: false,
        };
        assert!(!unsent.in_doubt());
        let sent = crate::repositories::span::SpanInsertError {
            error: ClickhouseError::Database(clickhouse::error::Error::Network("reset".into())),
            sent: true,
        };
        assert!(sent.in_doubt());
    }

    #[test]
    fn only_a_settling_server_answer_settles_a_write() {
        // The server ran the insert to its end and refused it.
        assert!(
            !answered(
                "Code: 395. DB::Exception: injected failure: while executing 'FUNCTION throwIf(1)'. \
                 (FUNCTION_THROW_IF_VALUE_IS_NON_ZERO) (version 25.3.1.1 (official build))"
            )
            .write_in_doubt()
        );
        assert!(
            !answered("241").write_in_doubt(),
            "a bare code from the header"
        );
        // Answers that leave the rows to land later, or say nothing about them.
        assert!(
            answered("Code: 159. DB::Exception: Wait for async insert timeout exceeded")
                .write_in_doubt()
        );
        assert!(answered("Code: 319. DB::Exception: Unknown status of insert").write_in_doubt());
        assert!(
            answered("504 Gateway Timeout").write_in_doubt(),
            "a proxy, not the server"
        );
        assert!(ClickhouseError::Database(clickhouse::error::Error::TimedOut).write_in_doubt());
        assert!(
            ClickhouseError::Database(clickhouse::error::Error::Network("reset".into()))
                .write_in_doubt()
        );
    }

    #[test]
    fn test_connection_error_display() {
        let err = ClickhouseError::Connection("connection refused".to_string());
        assert_eq!(err.to_string(), "Connection error: connection refused");
    }

    #[test]
    fn test_timeout_error_display() {
        let err = ClickhouseError::Timeout { timeout_secs: 30 };
        assert_eq!(err.to_string(), "Query timeout after 30s");
    }

    #[test]
    fn test_io_error_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let ch_err: ClickhouseError = io_err.into();
        assert!(ch_err.to_string().contains("file not found"));
    }
}
