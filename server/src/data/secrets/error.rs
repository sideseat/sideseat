use thiserror::Error;

#[derive(Error, Debug)]
pub enum SecretError {
    #[error("Secret not found: {0}")]
    NotFound(String),

    #[error("Secret backend error ({backend}): {message}")]
    Backend {
        backend: &'static str,
        message: String,
    },

    #[error("Secret backend is read-only ({backend})")]
    ReadOnly { backend: &'static str },

    #[error("Secret serialization error: {0}")]
    Serialization(String),

    #[error("Secret configuration error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
}

impl SecretError {
    pub fn backend(backend: &'static str, msg: impl Into<String>) -> Self {
        Self::Backend {
            backend,
            message: msg.into(),
        }
    }
}
