//! Checks that a model-provider credential is accepted, with the cheapest authenticated call the
//! provider offers.
//!
//! Most providers list models, which proves the credential without generating tokens or requiring
//! access to any particular model. Where listing is unavailable or does not authenticate, the probe
//! asks for a one-token completion instead, and a "model not found" answer still counts as success:
//! the provider had to accept the credential to say so.

mod aws;
mod azure;
mod google;
mod http;
mod providers;

#[cfg(test)]
mod tests;

use std::time::Duration;

use serde_json::Value;

/// A stored credential, as the probe needs it.
#[derive(Debug, Clone, Copy)]
pub struct ProbeTarget<'a> {
    /// The provider key, such as `anthropic`, `bedrock`, or `azure-ai-foundry`.
    pub provider: &'a str,
    /// The endpoint configured for providers without a fixed one.
    pub endpoint: Option<&'a str>,
    /// Provider-specific options: `auth_mode`, `region`, `project_id`, `deployment_name`, and so on.
    pub options: Option<&'a Value>,
    /// The secret: an API key, a token, or a JSON document of static credentials.
    pub secret: Option<&'a str>,
}

impl ProbeTarget<'_> {
    fn option(&self, key: &str) -> Option<&str> {
        self.options?.get(key)?.as_str()
    }

    fn secret(&self) -> &str {
        self.secret.unwrap_or("")
    }
}

/// Why a credential could not be confirmed.
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("unknown provider: {0}")]
    UnknownProvider(String),
    #[error("{0}")]
    Configuration(String),
    #[error("the provider rejected the credential ({status}): {message}")]
    Rejected { status: u16, message: String },
    #[error("the provider answered {status}: {message}")]
    Failed { status: u16, message: String },
    #[error("the provider is unreachable: {0}")]
    Unreachable(String),
    #[error("connection timed out")]
    TimedOut,
}

/// Probes `target` and returns a model the provider reported, when it reported one.
pub async fn probe(
    target: ProbeTarget<'_>,
    timeout: Duration,
) -> Result<Option<String>, ProbeError> {
    probe_with(target, timeout, &Endpoints::default()).await
}

async fn probe_with(
    target: ProbeTarget<'_>,
    timeout: Duration,
    endpoints: &Endpoints,
) -> Result<Option<String>, ProbeError> {
    tokio::time::timeout(timeout, providers::probe(target, endpoints))
        .await
        .map_err(|_| ProbeError::TimedOut)?
}

/// Where the probe sends requests. Production uses each provider's public origin; tests send every
/// request to one local server.
#[derive(Debug, Default, Clone)]
struct Endpoints {
    origin: Option<String>,
}

impl Endpoints {
    /// `url` with its scheme and host replaced by the override origin, when there is one.
    fn url(&self, url: &str) -> String {
        let Some(origin) = &self.origin else {
            return url.to_string();
        };
        let path_start = url
            .find("://")
            .and_then(|scheme| url[scheme + 3..].find('/').map(|path| scheme + 3 + path))
            .unwrap_or(url.len());
        format!("{origin}{}", &url[path_start..])
    }
}
