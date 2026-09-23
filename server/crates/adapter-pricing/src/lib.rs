//! External pricing catalogue adapter.

use std::time::Duration;

use async_trait::async_trait;
use sideseat_ports::pricing::{PricingCatalogueError, PricingCatalogueSource};

const LITELLM_PRICING_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const MAX_CATALOGUE_BYTES: usize = 16 * 1024 * 1024;
const USER_AGENT: &str = concat!("SideSeat/", env!("CARGO_PKG_VERSION"));

pub struct LiteLlmPricingSource {
    client: reqwest::Client,
    url: String,
}

impl LiteLlmPricingSource {
    pub fn new() -> Result<Self, PricingCatalogueError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(USER_AGENT)
            .build()
            .map_err(|error| PricingCatalogueError::new(error.to_string()))?;

        Ok(Self {
            client,
            url: LITELLM_PRICING_URL.to_string(),
        })
    }
}

#[async_trait]
impl PricingCatalogueSource for LiteLlmPricingSource {
    async fn fetch_catalogue(&self) -> Result<String, PricingCatalogueError> {
        let mut response = self
            .client
            .get(&self.url)
            .send()
            .await
            .map_err(|error| PricingCatalogueError::new(error.to_string()))?
            .error_for_status()
            .map_err(|error| PricingCatalogueError::new(error.to_string()))?;

        if response
            .content_length()
            .is_some_and(|length| length > MAX_CATALOGUE_BYTES as u64)
        {
            return Err(catalogue_too_large());
        }

        let mut body = Vec::with_capacity(
            response
                .content_length()
                .unwrap_or_default()
                .min(MAX_CATALOGUE_BYTES as u64) as usize,
        );
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| PricingCatalogueError::new(error.to_string()))?
        {
            checked_body_len(body.len(), chunk.len(), MAX_CATALOGUE_BYTES)?;
            body.extend_from_slice(&chunk);
        }

        String::from_utf8(body).map_err(|error| PricingCatalogueError::new(error.to_string()))
    }
}

fn checked_body_len(
    current: usize,
    additional: usize,
    maximum: usize,
) -> Result<usize, PricingCatalogueError> {
    current
        .checked_add(additional)
        .filter(|length| *length <= maximum)
        .ok_or_else(catalogue_too_large)
}

fn catalogue_too_large() -> PricingCatalogueError {
    PricingCatalogueError::new(format!(
        "pricing catalogue exceeds the {MAX_CATALOGUE_BYTES}-byte limit"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_source_targets_the_litellm_catalogue() {
        let source = LiteLlmPricingSource::new().expect("HTTP client");
        assert_eq!(source.url, LITELLM_PRICING_URL);
        assert_eq!(
            USER_AGENT,
            format!("SideSeat/{}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn catalogue_size_guard_accepts_the_limit() {
        assert_eq!(checked_body_len(6, 4, 10).unwrap(), 10);
    }

    #[test]
    fn catalogue_size_guard_rejects_oversized_and_overflowing_bodies() {
        assert!(checked_body_len(6, 5, 10).is_err());
        assert!(checked_body_len(usize::MAX, 1, usize::MAX).is_err());
    }
}
