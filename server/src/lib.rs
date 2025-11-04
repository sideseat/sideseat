pub mod api;
pub mod config;
pub mod embedded;
pub mod error;
pub mod middleware;
pub mod server;

pub use error::{Error, Result};

pub async fn run() -> Result<()> {
    server::start().await
}
