//! The server's stores and signals as `CoreApp::init` wires them for the embedded backends, in process, for the
//! harnesses that drive exports through the path an export takes: the storage gate's replay and the redelivery
//! tests. Included by `#[path]`, so every item here is used by each of them.

use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use sideseat_adapter_cache::CacheService;
use sideseat_core::config::{
    AnalyticsBackend, CacheBackendType, CacheConfig, EvictionPolicy, FilesConfig, RetentionConfig,
    StorageBackend, TransactionalBackend,
};
use sideseat_core::storage::{AppStorage, DataSubdir};
use sideseat_domain::files::FileService;
use sideseat_domain::storage_governance::StorageGovernanceService;
use sideseat_ingestion::received::ReceivedPayload;
use sideseat_ingestion::signals::{LogSignal, MetricsSignal, SignalContext, TraceSignal};
use sideseat_ingestion::staging::StagingService;
use sideseat_ingestion::traces::TracePipeline;
use sideseat_ports::clock::Clock;
use sideseat_ports::traits::{AnalyticsRepository, StorageGovernance, TransactionalRepository};
use sideseat_server::app::storage::{AnalyticsService, TransactionalService};

pub struct Stores {
    _cache: Arc<CacheService>,
    pub clock: Arc<dyn Clock>,
    pub database: Arc<TransactionalService>,
    pub analytics: Arc<AnalyticsService>,
    pub staging: Arc<StagingService>,
    pub governance: Arc<StorageGovernanceService>,
    pub pipeline: Arc<TracePipeline>,
    pub traces: TraceSignal,
    pub metrics: MetricsSignal,
    pub logs: LogSignal,
}

impl Stores {
    /// What the transports hand `export_signal` for one export to `project_id`.
    pub fn context<'a>(
        &'a self,
        project_id: &'a str,
        received: &'a ReceivedPayload,
    ) -> SignalContext<'a> {
        SignalContext {
            project_id,
            received,
            debug_path: None,
            clock: self.clock.as_ref(),
            staging: &self.staging,
            storage_governance: &self.governance,
        }
    }

    /// A project of `id` in the default organisation, as the projects API creates one.
    pub async fn create_project(&self, id: &str, name: &str, at: DateTime<Utc>) {
        let TransactionalService::Sqlite(sqlite) = self.database.as_ref() else {
            panic!("the harness runs on the embedded stores");
        };
        sqlx::query(
            "INSERT INTO projects (id, organization_id, name, created_at, updated_at) \
             VALUES (?, 'default', ?, ?, ?)",
        )
        .bind(id)
        .bind(name)
        .bind(at.timestamp())
        .bind(at.timestamp())
        .execute(sqlite.pool())
        .await
        .expect("a project");
    }
}

/// The stores in `root`, on `clock`, with a storage quota far above anything a harness sends, so everything sent is
/// stored.
pub async fn stores(root: &Path, clock: Arc<dyn Clock>) -> Stores {
    let app_storage = AppStorage::init_for_test(root.to_path_buf());
    std::fs::create_dir_all(app_storage.subdir(DataSubdir::FilesTemp)).expect("files temp dir");
    let cache = Arc::new(
        CacheService::new(&CacheConfig {
            backend: CacheBackendType::Memory,
            max_entries: 10_000,
            eviction_policy: EvictionPolicy::TinyLfu,
            redis_url: None,
        })
        .await
        .expect("cache"),
    );
    let database = Arc::new(
        TransactionalService::init(
            TransactionalBackend::Sqlite,
            &app_storage,
            None,
            Some(cache.clone()),
            Arc::clone(&clock),
        )
        .await
        .expect("sqlite"),
    );
    let analytics = Arc::new(
        AnalyticsService::init(
            AnalyticsBackend::Duckdb,
            &app_storage,
            None,
            Arc::clone(&clock),
        )
        .await
        .expect("duckdb"),
    );
    let database_port: Arc<dyn TransactionalRepository + Send + Sync> =
        Arc::from(database.repository());
    let analytics_port: Arc<dyn AnalyticsRepository + Send + Sync> =
        Arc::from(analytics.repository());
    let governance_port: Arc<dyn StorageGovernance + Send + Sync> =
        Arc::from(database.governance_repository());
    let quota = 1_u64 << 40;
    let governance = Arc::new(StorageGovernanceService::new(
        Arc::clone(&database_port),
        governance_port,
        Arc::clone(&analytics_port),
        Arc::clone(&clock),
        quota,
    ));
    let files = Arc::new(
        FileService::new_governed(
            FilesConfig {
                enabled: true,
                storage: StorageBackend::Filesystem,
                quota_bytes: quota,
                filesystem_path: None,
                s3: None,
            },
            app_storage.subdir(DataSubdir::FilesTemp),
            Arc::new(sideseat_adapter_blob_storage::FilesystemStorage::new(
                app_storage.subdir(DataSubdir::Files),
            )),
            Arc::clone(&database_port),
            cache.clone(),
            Arc::clone(&governance),
        )
        .await
        .expect("files"),
    );
    let topics = Arc::new(sideseat_messaging::TopicService::new(
        sideseat_adapter_topics::memory_backend(),
    ));
    let staging = Arc::new(StagingService::new(
        Arc::clone(files.storage()),
        Arc::clone(&database_port),
        Arc::clone(&analytics_port),
        Arc::clone(&clock),
        RetentionConfig::default(),
        5,
    ));
    let pipeline = Arc::new(
        TracePipeline::new(
            Arc::clone(&analytics_port),
            Arc::new(
                sideseat_domain::pricing::PricingService::init_for_test().expect("offline pricing"),
            ),
            Arc::clone(&topics),
            Arc::clone(&files),
            Arc::clone(&staging),
        )
        .with_storage_governance(Arc::clone(&governance)),
    );
    let trace_topic = Arc::new(
        topics.stream_topic::<sideseat_ingestion::staging::StagedPayloadRef>(
            sideseat_core::constants::TOPIC_TRACES,
            sideseat_ingestion::staging::StagedPayloadRef::partition_key,
        ),
    );
    Stores {
        _cache: cache,
        clock,
        traces: TraceSignal::new(trace_topic, Some(Arc::clone(&pipeline))),
        metrics: MetricsSignal::new(Arc::clone(&analytics_port), Arc::clone(&database_port)),
        logs: LogSignal::new(analytics_port, database_port),
        database,
        analytics,
        staging,
        governance,
        pipeline,
    }
}
