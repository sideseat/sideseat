//! Pricing catalogue source boundary.

use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Error)]
#[error("{message}")]
pub struct PricingCatalogueError {
    message: String,
}

impl PricingCatalogueError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[async_trait]
pub trait PricingCatalogueSource: Send + Sync {
    async fn fetch_catalogue(&self) -> Result<String, PricingCatalogueError>;
}
