//! External pricing catalogue adapter.

use std::time::Duration;

use async_trait::async_trait;
use sideseat_ports::pricing::{PricingCatalogueError, PricingCatalogueSource};

const LITELLM_PRICING_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

pub struct LiteLlmPricingSource {
    client: reqwest::Client,
    url: String,
}

impl LiteLlmPricingSource {
    pub fn new() -> Result<Self, PricingCatalogueError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("SideSeat/1.0")
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
        let response = self
            .client
            .get(&self.url)
            .send()
            .await
            .map_err(|error| PricingCatalogueError::new(error.to_string()))?
            .error_for_status()
            .map_err(|error| PricingCatalogueError::new(error.to_string()))?;

        response
            .text()
            .await
            .map_err(|error| PricingCatalogueError::new(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_source_targets_the_litellm_catalogue() {
        let source = LiteLlmPricingSource::new().expect("HTTP client");
        assert_eq!(source.url, LITELLM_PRICING_URL);
    }
}
