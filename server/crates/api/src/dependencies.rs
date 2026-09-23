//! Driver-independent dependencies shared by API route state.

use std::sync::Arc;

use sideseat_core::core::config::AppConfig;
use sideseat_core::core::storage::AppStorage;
use sideseat_domain::files::FileService;
use sideseat_domain::pricing::PricingService;
use sideseat_domain::providers::{CredentialConnectionTester, CredentialService};
use sideseat_domain::rate_limit::RateLimiter;
use sideseat_domain::staging::StagingService;
use sideseat_domain::storage_governance::StorageGovernanceService;
use sideseat_domain::topics::TopicService;
use sideseat_ports::clock::Clock;
use sideseat_ports::registrations::RegistrationStore;
use tokio::sync::watch;

use super::auth::AuthManager;

pub type AnalyticsStore = dyn sideseat_ports::traits::AnalyticsRepository + Send + Sync + 'static;
pub type TransactionalStore =
    dyn sideseat_ports::traits::TransactionalRepository + Send + Sync + 'static;
pub type SharedCache = dyn sideseat_ports::cache::CacheStore + 'static;

/// Everything the transport layer needs, expressed only in inward-facing types.
pub struct ApiDependencies {
    pub config: AppConfig,
    pub storage: AppStorage,
    pub database: Arc<TransactionalStore>,
    pub analytics: Arc<AnalyticsStore>,
    pub pricing: Arc<PricingService>,
    pub auth: Arc<AuthManager>,
    pub topics: Arc<TopicService>,
    pub files: Arc<FileService>,
    pub staging: Arc<StagingService>,
    pub storage_governance: Arc<StorageGovernanceService>,
    pub cache: Arc<SharedCache>,
    pub rate_limiter: Arc<RateLimiter>,
    pub credentials: Arc<CredentialService>,
    pub credential_tester: Arc<dyn CredentialConnectionTester>,
    pub registrations: Arc<dyn RegistrationStore>,
    pub api_key_secret: Vec<u8>,
    pub clock: Arc<dyn Clock>,
    pub shutdown_rx: watch::Receiver<bool>,
}
