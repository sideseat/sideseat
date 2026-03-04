use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("API error (status {status}): {message}")]
    Api { status: u16, message: String },

    #[error("Context window exceeded: {0}")]
    ContextWindowExceeded(String),

    #[error("Rate limited: {0}")]
    RateLimited(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Stream error: {0}")]
    Stream(String),

    #[error("Invalid configuration: {0}")]
    Config(String),

    #[error("Unsupported feature: {0}")]
    Unsupported(String),
}

impl From<reqwest::Error> for ProviderError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_status() {
            let status = e.status().map(|s| s.as_u16()).unwrap_or(0);
            if status == 429 {
                return ProviderError::RateLimited(e.to_string());
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
            Self::Network(_) | Self::RateLimited(_) => true,
            Self::Api { status, .. } => *status >= 500,
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

    /// Returns the HTTP status code for API errors.
    pub fn status_code(&self) -> Option<u16> {
        if let Self::Api { status, .. } = self {
            Some(*status)
        } else {
            None
        }
    }
}
