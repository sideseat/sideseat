/// Why [`init`](crate::init) could not configure telemetry, or why a [`Session`](crate::Session)
/// could not be built.
///
/// Nothing after `init` fails because of telemetry: export problems are reported by the boolean
/// results of [`SideSeat::flush`](crate::SideSeat::flush) and
/// [`SideSeat::shutdown`](crate::SideSeat::shutdown).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An option or environment variable has an invalid value.
    #[error("invalid SideSeat configuration: {0}")]
    Configuration(String),
    /// `init` already configured this process with different settings.
    #[error("init was already called with different settings; call shutdown() first")]
    AlreadyInitialized,
    /// A session or user id is empty, which would silently merge unrelated conversations.
    #[error("the {0} must not be empty")]
    EmptyId(&'static str),
    /// An OTLP exporter could not be built.
    #[error("could not build the {signal} exporter: {message}")]
    Exporter {
        signal: &'static str,
        message: String,
    },
}
