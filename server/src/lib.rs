pub mod api;
pub mod config;
pub mod db;
pub mod embedded;
pub mod error;
pub mod middleware;
pub mod server;

// Empty module declarations
pub mod a2a;
pub mod mcp;
pub mod otel;
pub mod prompts;
pub mod proxy;

pub use error::{Error, Result};

pub async fn run() -> Result<()> {
    server::start().await
}
