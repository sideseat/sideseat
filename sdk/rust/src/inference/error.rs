use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum ProviderError {
    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("API error (status {status}): {message}")]
    Api { status: u16, message: String },

    #[error("Context window exceeded: {0}")]
    ContextWindowExceeded(String),

    /// Request timed out.
    ///
    /// - `ms: None` — timeout reported by `reqwest` (no explicit duration available).
    /// - `ms: Some(n)` — explicit timeout set via [`crate::ProviderConfig`]'s `timeout_ms` field;
    ///   the request was cancelled after `n` milliseconds.
    #[error("Request timed out")]
    Timeout { ms: Option<u64> },

    #[error("Model not found: {model}")]
    ModelNotFound { model: String },

    #[error("Too many requests: {message}")]
    TooManyRequests {
        message: String,
        retry_after_secs: Option<u64>,
    },

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Stream error: {0}")]
    Stream(String),

    #[error("Missing configuration: {0}")]
    MissingConfig(String),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Unsupported feature: {0}")]
    Unsupported(String),

    #[error("Content filtered: {0}")]
    ContentFilterViolation(String),

    /// The provider returned a response with no usable content of the expected type.
    /// This is a logical error (not a network failure) and is not retryable.
    #[error("Empty response: {0}")]
    EmptyResponse(String),
}

impl From<reqwest::Error> for ProviderError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            return ProviderError::Timeout { ms: None };
        }
        if e.is_status() {
            let status = e.status().map(|s| s.as_u16()).unwrap_or(0);
            if status == 429 {
                return ProviderError::TooManyRequests {
                    message: e.to_string(),
                    retry_after_secs: None,
                };
            }
            ProviderError::Api {
                status,
                message: e.to_string(),
            }
        } else {
            ProviderError::Network(e.to_string())
        }
    }
}

impl From<serde_json::Error> for ProviderError {
    fn from(e: serde_json::Error) -> Self {
        ProviderError::Serialization(e.to_string())
    }
}

impl ProviderError {
    /// Returns true for transient errors that may succeed on retry.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Network(_) | Self::Timeout { .. } | Self::TooManyRequests { .. } => true,
            // 5xx are server errors; 424 (Failed Dependency) is used by Bedrock for
            // transient infrastructure issues ("try your request again").
            Self::Api { status, .. } => *status >= 500 || *status == 424,
            _ => false,
        }
    }

    /// Returns true for client-side errors (4xx).
    pub fn is_client_error(&self) -> bool {
        matches!(self, Self::Api { status, .. } if (400..500).contains(status))
    }

    /// Returns true for authentication/authorization errors.
    pub fn is_auth_error(&self) -> bool {
        match self {
            Self::Auth(_) => true,
            Self::Api { status, .. } => *status == 401 || *status == 403,
            _ => false,
        }
    }

    /// Returns true when this error indicates an operation is not supported by the provider.
    pub fn is_unsupported(&self) -> bool {
        matches!(self, Self::Unsupported(_))
    }

    /// Returns the HTTP status code for API errors.
    pub fn status_code(&self) -> Option<u16> {
        if let Self::Api { status, .. } = self {
            Some(*status)
        } else {
            None
        }
    }
}
