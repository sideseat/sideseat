//! Core application infrastructure

pub(crate) mod banner;
pub mod cli;
pub mod config;
pub mod constants;
pub mod secret;
pub mod shutdown;
pub mod storage;
pub(crate) mod update;

pub use crate::app::CoreApp;
pub use cli::{CliConfig, Commands};
pub use config::{AppConfig, AuthConfig, ServerConfig};
pub use secret::{Secret, SecretBackend, SecretManager};
pub use storage::{AppStorage, DataSubdir};

// Re-export service enums from data layer
pub use crate::data::{AnalyticsService, TransactionalService};
// Re-export backend-specific services for direct access when needed
pub use crate::data::{DuckdbService, SqliteService};

pub use crate::domain::pricing::{PricingService, SpanCostInput, SpanCostOutput};
pub use shutdown::ShutdownService;

// Re-export topic types from data::topics for backward compatibility
// The canonical location is now data::topics
pub use crate::data::topics::{
    Publisher, Subscriber, Topic, TopicConfig, TopicError, TopicMessage, TopicService,
};
