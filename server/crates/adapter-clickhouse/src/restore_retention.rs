use std::sync::Arc;

use sideseat_core::config::RetentionConfig;
use sideseat_ports::blobs::RetentionFileReconciler;
use sideseat_ports::traits::{StorageGovernance, TransactionalRepository};

use crate::{ClickhouseError, ClickhouseService, retention};

impl ClickhouseService {
    /// Run explicit age and count retention to a fixed point before restored rows are served.
    ///
    /// ClickHouse age deletion is one synchronous `ALTER ... DELETE` predicate (`mutations_sync = 2`).
    /// Count retention remains bounded per call, so the restore path repeats it until no candidate remains.
    pub async fn run_retention_to_completion(
        self: &Arc<Self>,
        config: &RetentionConfig,
        quota_bytes: u64,
        file_service: Option<Arc<dyn RetentionFileReconciler>>,
        database: Arc<dyn TransactionalRepository + Send + Sync>,
        governance: Arc<dyn StorageGovernance + Send + Sync>,
    ) -> Result<(), ClickhouseError> {
        let projects = governance
            .storage_project_ids(usize::MAX)
            .await
            .map_err(|error| ClickhouseError::Connection(error.to_string()))?;
        let spans_table = self.delete_table("otel_spans");
        let metrics_table = self.delete_table("otel_metrics");
        let logs_table = self.delete_table("otel_logs");
        let on_cluster = self.on_cluster_clause();

        for project_id in projects {
            let owner = format!(
                "clickhouse-restore-retention:{}:{}",
                std::process::id(),
                self.clock.now().timestamp_micros()
            );
            let lock_now = self.clock.now();
            let acquired = governance
                .acquire_project_maintenance(
                    &project_id,
                    &owner,
                    lock_now,
                    lock_now + chrono::TimeDelta::hours(2),
                )
                .await
                .map_err(|error| ClickhouseError::Connection(error.to_string()))?;
            if !acquired {
                return Err(ClickhouseError::Connection(format!(
                    "project {project_id} is already under maintenance"
                )));
            }

            let result: Result<(), ClickhouseError> = async {
                let held = governance
                    .active_project_hold(&project_id, self.clock.now())
                    .await
                    .map_err(|error| ClickhouseError::Connection(error.to_string()))?
                    .is_some();
                if held {
                    tracing::info!(%project_id, "Restore retention kept data under legal hold");
                    return Ok(());
                }

                if let Some(max_age_minutes) = config.max_age_minutes {
                    let now = self.clock.now();
                    let cutoff = now
                        - chrono::Duration::minutes(
                            i64::try_from(max_age_minutes).unwrap_or(i64::MAX),
                        );
                    retention::run_retention(
                        &self.tenant_client(&project_id),
                        &spans_table,
                        &metrics_table,
                        &logs_table,
                        &on_cluster,
                        project_id.as_str(),
                        cutoff,
                        now,
                    )
                    .await?;
                }

                if let Some(max_spans) = config.max_spans {
                    loop {
                        let deleted = Self::trim_project_to_span_limit(
                            self,
                            max_spans,
                            quota_bytes,
                            &database,
                            &governance,
                            file_service.as_ref(),
                            &project_id,
                        )
                        .await
                        .map_err(ClickhouseError::Connection)?;
                        if deleted == 0 {
                            break;
                        }
                    }
                }
                Ok(())
            }
            .await;

            let release = governance
                .release_project_maintenance(&project_id, &owner)
                .await
                .map_err(|error| ClickhouseError::Connection(error.to_string()));
            result?;
            release?;
        }
        Ok(())
    }

    /// Close the connection gracefully (no-op for ClickHouse HTTP client)
    pub async fn close(&self) {
        tracing::debug!("ClickHouse connection closed");
    }
}
