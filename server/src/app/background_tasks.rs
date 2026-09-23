//! Registration of long-running application maintenance and ingestion tasks.

use std::sync::Arc;

use anyhow::Result;

use sideseat_core::constants::TOPIC_TRACES;
use sideseat_ingestion::staging::StagedPayloadRef;

use super::CoreApp;

impl CoreApp {
    pub(super) async fn start_background_tasks(&self) -> Result<()> {
        self.shutdown
            .register(
                self.secrets
                    .start_health_check_task(self.shutdown.subscribe()),
            )
            .await;

        self.shutdown
            .register(
                self.database
                    .start_checkpoint_task(self.shutdown.subscribe()),
            )
            .await;

        self.shutdown
            .register(
                self.analytics
                    .start_checkpoint_task(self.shutdown.subscribe()),
            )
            .await;

        // ClickHouse reports cross-month duplicate residuals; DuckDB has no partition check.
        if let Some(handle) = self
            .analytics
            .start_consistency_check_task(self.shutdown.subscribe())
        {
            self.shutdown.register(handle).await;
        }

        if let Some(handle) = self.analytics.start_retention_task(
            self.config.otel.retention.clone(),
            self.config.files.quota_bytes,
            self.shutdown.subscribe(),
            Some(Arc::clone(&self.files)),
            Arc::clone(&self.database),
            Arc::from(self.database.governance_repository()),
        ) {
            self.shutdown.register(handle).await;
        }

        if let Some(handle) = self
            .pricing
            .start_sync_task(self.config.pricing.sync_hours, self.shutdown.subscribe())
        {
            self.shutdown.register(handle).await;
        }

        // Recover deletion claims abandoned by a crashed worker.
        self.shutdown
            .register(sideseat_domain::cleanup::start_claim_recovery_task(
                Arc::clone(&self.database_port),
                Arc::clone(&self.analytics_port),
                Arc::clone(&self.files),
                self.shutdown.subscribe(),
            ))
            .await;

        let traces_topic = self
            .topics
            .stream_topic::<StagedPayloadRef>(TOPIC_TRACES, StagedPayloadRef::partition_key);

        let pipeline = Arc::new(
            sideseat_ingestion::traces::TracePipeline::new(
                Arc::from(self.analytics.repository()),
                self.pricing.clone(),
                self.topics.clone(),
                self.files.clone(),
                Arc::clone(&self.staging),
            )
            .with_storage_governance(Arc::clone(&self.storage_governance)),
        );

        self.shutdown
            .register(Arc::clone(&pipeline).start(traces_topic, self.shutdown.subscribe()))
            .await;
        self.shutdown
            .register(sideseat_ingestion::staging::start_staging_sweep(
                Arc::clone(&self.staging),
                pipeline,
                self.shutdown.subscribe(),
            ))
            .await;
        self.shutdown
            .register(Arc::clone(&self.storage_governance).start(self.shutdown.subscribe()))
            .await;
        self.shutdown
            .register(
                Arc::new(
                    sideseat_domain::content_bodies::ContentBodyService::from_file_service(
                        &self.files,
                    ),
                )
                .start_backfill_task(
                    Arc::clone(&self.analytics_port),
                    Arc::clone(&self.clock),
                    self.shutdown.subscribe(),
                ),
            )
            .await;

        // Metrics persist in their request path, so they do not need a background pipeline.
        tracing::debug!("Background tasks started");
        Ok(())
    }
}
