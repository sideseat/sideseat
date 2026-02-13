//! API server and routes

pub mod auth;
mod embedded;
pub mod extractors;
mod mcp;
pub mod middleware;
pub mod openapi;
pub mod rate_limit;
pub mod routes;
mod server;
pub mod types;

pub use auth::AuthManager;
pub use routes::otlp_collector::OtlpGrpcServer;
pub use server::ApiServer;
