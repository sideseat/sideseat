pub mod api;
pub mod auth;
pub mod core;
pub mod error;
pub mod otel;
pub mod server;

pub use auth::AuthManager;
pub use core::CliConfig;
pub use error::{Error, Result};

pub async fn run(cli_config: CliConfig) -> Result<()> {
    server::start(cli_config).await
}
