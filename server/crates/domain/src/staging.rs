//! Durable OTLP staging shared by every signal lifecycle.
//!
//! The blob is written before its registry row. That ordering makes an
//! acknowledged payload discoverable even if the queue loses every entry. A
//! payload leaves staging only after strict producer-content confirmation, or
//! after every absent record is explained by the same deletion/retention rules
//! as ingestion.

use std::sync::Arc;

use chrono::TimeDelta;
use opentelemetry_proto::tonic::collector::{
    logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
    trace::v1::ExportTraceServiceRequest,
};
use prost::Message;
use sideseat_core::core::config::RetentionConfig;
use sideseat_ports::blobs::{FileStorage, FileStorageError};
use sideseat_ports::clock::Clock;
use sideseat_ports::traits::{AnalyticsRepository, DeletionScope, TransactionalRepository};
use sideseat_ports::types::{ProjectId, StagedPayload, StagedRecord, StagedSignal};
use thiserror::Error;
use uuid::Uuid;

use crate::traces::{IngestOutcome, TracePipeline};

/// Compact durable-queue value. Payload bytes remain in the blob store.
#[derive(Clone, PartialEq, Eq, Message)]
pub struct StagedPayloadRef {
    #[prost(string, tag = "1")]
    pub id: String,
    #[prost(string, tag = "2")]
    pub partition_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagingDisposition {
    Confirmed,
    DeliberatelyAbsent,
    Pending,
}

#[derive(Debug, Error)]
pub enum StagingError {
    #[error("staging registry operation failed: {0}")]
    Registry(String),
    #[error("staging blob operation failed: {0}")]
    Blob(String),
    #[error("staged payload {0} has no blob")]
    MissingBlob(String),
}

/// Owns the blob/registry lifecycle and the signal-specific confirmation
/// predicates. It deliberately has no time-to-live.
pub struct StagingService {
    storage: Arc<dyn FileStorage>,
    database: Arc<dyn TransactionalRepository + Send + Sync>,
    analytics: Arc<dyn AnalyticsRepository + Send + Sync>,
    clock: Arc<dyn Clock>,
    retention: RetentionConfig,
    redrive_cap: u32,
}

impl StagingService {
    pub fn new(
        storage: Arc<dyn FileStorage>,
        database: Arc<dyn TransactionalRepository + Send + Sync>,
        analytics: Arc<dyn AnalyticsRepository + Send + Sync>,
        clock: Arc<dyn Clock>,
        retention: RetentionConfig,
        redrive_cap: u32,
    ) -> Self {
        Self {
            storage,
            database,
            analytics,
            clock,
            retention,
            redrive_cap: redrive_cap.max(1),
        }
    }

    pub async fn stage(
        &self,
        project_id: &str,
        signal: StagedSignal,
        bytes: &[u8],
        records: Vec<StagedRecord>,
        partition_key: String,
    ) -> Result<StagedPayloadRef, StagingError> {
        let id = Uuid::new_v4().to_string();
        let project_id = ProjectId::from(project_id);
        let mut hasher = blake3::Hasher::new();
        hasher.update(id.as_bytes());
        hasher.update(project_id.as_bytes());
        hasher.update(signal.as_str().as_bytes());
        hasher.update(bytes);
        let blob_hash = hasher.finalize().to_hex().to_string();

        self.storage
            .store(&project_id, &blob_hash, bytes)
            .await
            .map_err(blob_error)?;

        let payload = StagedPayload {
            id: id.clone(),
            project_id,
            signal,
            blob_hash,
            byte_len: bytes.len() as u64,
            created_at: self.clock.now(),
            redrive_attempts: 0,
            unconfirmed: false,
            records,
        };
        if let Err(error) = self.database.create_staged_payload(&payload).await {
            if let Err(cleanup_error) = self
                .storage
                .delete(&payload.project_id, &payload.blob_hash)
                .await
            {
                tracing::error!(
                    staged_payload_id = %payload.id,
                    %cleanup_error,
                    "Could not remove a staging blob after its registry write failed"
                );
            }
            return Err(StagingError::Registry(error.to_string()));
        }

        Ok(StagedPayloadRef { id, partition_key })
    }

    pub async fn load(&self, id: &str) -> Result<Option<(StagedPayload, Vec<u8>)>, StagingError> {
        let Some(payload) = self
            .database
            .get_staged_payload(id)
            .await
            .map_err(|error| StagingError::Registry(error.to_string()))?
        else {
            return Ok(None);
        };
        let bytes = match self
            .storage
            .get(&payload.project_id, &payload.blob_hash)
            .await
        {
            Ok(bytes) => bytes,
            Err(FileStorageError::NotFound { .. }) => {
                return Err(StagingError::MissingBlob(payload.id));
            }
            Err(error) => return Err(blob_error(error)),
        };
        Ok(Some((payload, bytes)))
    }

    pub async fn pending(&self, limit: usize) -> Result<Vec<StagedPayload>, StagingError> {
        self.database
            .pending_staged_payloads(limit)
            .await
            .map_err(|error| StagingError::Registry(error.to_string()))
    }

    /// Check strict content equality. On a miss, prove every missing record was
    /// intentionally removed before declaring the payload terminal.
    pub async fn disposition(
        &self,
        payload: &StagedPayload,
    ) -> Result<StagingDisposition, StagingError> {
        if self.all_confirmed(payload).await? {
            return Ok(StagingDisposition::Confirmed);
        }

        if !self
            .database
            .project_accepts_writes(payload.project_id.as_str())
            .await
            .map_err(|error| StagingError::Registry(error.to_string()))?
        {
            return Ok(StagingDisposition::DeliberatelyAbsent);
        }

        let mut saw_absence = false;
        for record in &payload.records {
            if self.record_confirmed(&payload.project_id, record).await? {
                continue;
            }
            if self
                .record_deliberately_absent(&payload.project_id, record)
                .await?
            {
                saw_absence = true;
                continue;
            }
            return Ok(StagingDisposition::Pending);
        }

        Ok(if saw_absence {
            StagingDisposition::DeliberatelyAbsent
        } else {
            StagingDisposition::Confirmed
        })
    }

    /// Release terminal payloads. Returns the observed disposition.
    pub async fn settle(
        &self,
        payload: &StagedPayload,
    ) -> Result<StagingDisposition, StagingError> {
        let disposition = self.disposition(payload).await?;
        if disposition != StagingDisposition::Pending {
            self.release(payload).await?;
        }
        Ok(disposition)
    }

    /// Record a failed write/read-back cycle. At the configured cap the row is
    /// quarantined and the bytes remain held indefinitely.
    pub async fn note_failed_attempt(&self, id: &str) -> Result<bool, StagingError> {
        let attempts = self
            .database
            .increment_staged_redrive_attempts(id)
            .await
            .map_err(|error| StagingError::Registry(error.to_string()))?;
        if attempts >= self.redrive_cap {
            self.database
                .mark_staged_unconfirmed(id)
                .await
                .map_err(|error| StagingError::Registry(error.to_string()))?;
            tracing::error!(
                staged_payload_id = id,
                attempts,
                "Staged OTLP payload exhausted its redrive cap and remains held as unconfirmed"
            );
            return Ok(true);
        }
        Ok(false)
    }

    pub async fn release(&self, payload: &StagedPayload) -> Result<(), StagingError> {
        // Registry first: a crash after it leaves an unreferenced blob, while
        // the opposite order leaves a live registry row pointing at no bytes.
        self.database
            .delete_staged_payload(&payload.id)
            .await
            .map_err(|error| StagingError::Registry(error.to_string()))?;
        if let Err(error) = self
            .storage
            .delete(&payload.project_id, &payload.blob_hash)
            .await
        {
            // The terminal fact is the registry deletion. A failed physical
            // delete leaves reclaimable garbage, not an unconfirmed delivery.
            tracing::error!(
                staged_payload_id = %payload.id,
                blob_hash = %payload.blob_hash,
                %error,
                "Could not remove a terminal staging blob; it is now an orphan"
            );
        }
        Ok(())
    }

    /// Reconcile one page of registry rows. This repairs both queue loss and
    /// inline requests interrupted after their durable staging write.
    pub async fn redrive_once(
        &self,
        trace_pipeline: &TracePipeline,
        limit: usize,
    ) -> Result<usize, StagingError> {
        let payloads = self.pending(limit).await?;
        let count = payloads.len();
        for payload in payloads {
            match self.disposition(&payload).await? {
                StagingDisposition::Confirmed | StagingDisposition::DeliberatelyAbsent => {
                    self.release(&payload).await?;
                    continue;
                }
                StagingDisposition::Pending => {}
            }

            let Some((current, bytes)) = self.load(&payload.id).await? else {
                continue;
            };
            let write_ok = match current.signal {
                StagedSignal::Traces => match ExportTraceServiceRequest::decode(bytes.as_slice()) {
                    Ok(request) => !matches!(
                        trace_pipeline.ingest_now(&request).await,
                        IngestOutcome::Failed
                    ),
                    Err(error) => {
                        tracing::error!(
                            staged_payload_id = %current.id,
                            %error,
                            "Could not decode a staged trace payload during redrive"
                        );
                        false
                    }
                },
                StagedSignal::Metrics => {
                    match ExportMetricsServiceRequest::decode(bytes.as_slice()) {
                        Ok(request) => crate::metrics::ingest(
                            &request,
                            self.analytics.as_ref(),
                            self.database.as_ref(),
                        )
                        .await
                        .is_ok(),
                        Err(error) => {
                            tracing::error!(
                                staged_payload_id = %current.id,
                                %error,
                                "Could not decode a staged metrics payload during redrive"
                            );
                            false
                        }
                    }
                }
                StagedSignal::Logs => match ExportLogsServiceRequest::decode(bytes.as_slice()) {
                    Ok(request) => crate::logs::ingest(
                        &request,
                        self.analytics.as_ref(),
                        self.database.as_ref(),
                        current.created_at,
                    )
                    .await
                    .is_ok(),
                    Err(error) => {
                        tracing::error!(
                            staged_payload_id = %current.id,
                            %error,
                            "Could not decode a staged logs payload during redrive"
                        );
                        false
                    }
                },
            };

            if write_ok && self.settle(&current).await? != StagingDisposition::Pending {
                continue;
            }
            let _ = self.note_failed_attempt(&current.id).await?;
        }
        Ok(count)
    }

    async fn all_confirmed(&self, payload: &StagedPayload) -> Result<bool, StagingError> {
        match payload.signal {
            StagedSignal::Traces => {
                let records = payload
                    .records
                    .iter()
                    .filter_map(|record| match record {
                        StagedRecord::Span {
                            trace_id,
                            span_id,
                            content_digest,
                            ..
                        } => Some((trace_id.clone(), span_id.clone(), content_digest.clone())),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                self.analytics
                    .spans_match_content(&payload.project_id, &records)
                    .await
            }
            StagedSignal::Metrics => {
                let records = payload
                    .records
                    .iter()
                    .filter_map(|record| match record {
                        StagedRecord::Metric {
                            datapoint_id,
                            content_digest,
                            ..
                        } => Some((datapoint_id.clone(), content_digest.clone())),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                self.analytics
                    .metrics_match_content(&payload.project_id, &records)
                    .await
            }
            StagedSignal::Logs => {
                let records = payload
                    .records
                    .iter()
                    .filter_map(|record| match record {
                        StagedRecord::Log {
                            log_digest,
                            ordinal,
                            ..
                        } => Some((log_digest.clone(), *ordinal)),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                self.analytics
                    .logs_match_content(&payload.project_id, &records)
                    .await
            }
        }
        .map_err(|error| StagingError::Registry(error.to_string()))
    }

    async fn record_confirmed(
        &self,
        project_id: &ProjectId,
        record: &StagedRecord,
    ) -> Result<bool, StagingError> {
        match record {
            StagedRecord::Span {
                trace_id,
                span_id,
                content_digest,
                ..
            } => {
                self.analytics
                    .spans_match_content(
                        project_id,
                        &[(trace_id.clone(), span_id.clone(), content_digest.clone())],
                    )
                    .await
            }
            StagedRecord::Metric {
                datapoint_id,
                content_digest,
                ..
            } => {
                self.analytics
                    .metrics_match_content(
                        project_id,
                        &[(datapoint_id.clone(), content_digest.clone())],
                    )
                    .await
            }
            StagedRecord::Log {
                log_digest,
                ordinal,
                ..
            } => {
                self.analytics
                    .logs_match_content(project_id, &[(log_digest.clone(), *ordinal)])
                    .await
            }
        }
        .map_err(|error| StagingError::Registry(error.to_string()))
    }

    async fn record_deliberately_absent(
        &self,
        project_id: &ProjectId,
        record: &StagedRecord,
    ) -> Result<bool, StagingError> {
        if let Some(max_age) = self.retention.max_age_minutes {
            let cutoff = self.clock.now()
                - TimeDelta::try_minutes(i64::try_from(max_age).unwrap_or(i64::MAX))
                    .unwrap_or(TimeDelta::MAX);
            if record.timestamp() < cutoff {
                return Ok(true);
            }
        }

        let journaled = match record {
            StagedRecord::Span {
                trace_id, span_id, ..
            } => {
                self.database
                    .deletion_is_journaled(project_id, DeletionScope::Span, trace_id, Some(span_id))
                    .await
            }
            StagedRecord::Log {
                trace_id: Some(trace_id),
                span_id,
                ..
            } => {
                self.database
                    .deletion_is_journaled(
                        project_id,
                        if span_id.is_some() {
                            DeletionScope::Span
                        } else {
                            DeletionScope::Trace
                        },
                        trace_id,
                        span_id.as_deref(),
                    )
                    .await
            }
            StagedRecord::Metric { .. } | StagedRecord::Log { trace_id: None, .. } => {
                return Ok(false);
            }
        };
        journaled.map_err(|error| StagingError::Registry(error.to_string()))
    }
}

/// Periodically repairs queue loss and writes interrupted after staging.
pub fn start_staging_sweep(
    staging: Arc<StagingService>,
    trace_pipeline: Arc<TracePipeline>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                }
                _ = interval.tick() => {
                    if let Err(error) = staging.redrive_once(trace_pipeline.as_ref(), 100).await {
                        tracing::error!(%error, "Staged-payload sweep failed");
                    }
                }
            }
        }
    })
}

fn blob_error(error: FileStorageError) -> StagingError {
    StagingError::Blob(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use sideseat_adapter_blob_storage::FilesystemStorage;
    use sideseat_adapter_duckdb::{DuckdbRepository, DuckdbService};
    use sideseat_adapter_sqlite::{SqliteRepository, SqliteService};
    use sideseat_core::core::storage::AppStorage;
    use sideseat_ports::types::NormalizedMetric;

    #[derive(Debug)]
    struct TestClock;

    impl Clock for TestClock {
        fn now(&self) -> chrono::DateTime<Utc> {
            Utc.timestamp_opt(1_704_067_200, 0).single().unwrap()
        }
    }

    #[tokio::test]
    async fn confirmed_retries_release_and_cap_exhaustion_holds_bytes() {
        let temp = tempfile::TempDir::new().unwrap();
        let app_storage = AppStorage::init_for_test(temp.path().to_path_buf());
        tokio::fs::create_dir_all(temp.path().join("duckdb"))
            .await
            .unwrap();
        let clock: Arc<dyn Clock> = Arc::new(TestClock);
        let sqlite = Arc::new(
            SqliteService::init(&app_storage, Arc::clone(&clock))
                .await
                .unwrap(),
        );
        let database: Arc<dyn TransactionalRepository + Send + Sync> =
            Arc::new(SqliteRepository(sqlite));
        let duck = Arc::new(
            DuckdbService::init(&app_storage, Arc::clone(&clock))
                .await
                .unwrap(),
        );
        let analytics: Arc<dyn AnalyticsRepository + Send + Sync> =
            Arc::new(DuckdbRepository(duck));
        let storage: Arc<dyn FileStorage> =
            Arc::new(FilesystemStorage::new(temp.path().join("staging")));
        let service = StagingService::new(
            Arc::clone(&storage),
            Arc::clone(&database),
            Arc::clone(&analytics),
            Arc::clone(&clock),
            RetentionConfig::default(),
            2,
        );

        let user = database
            .create_user("staging@example.test", None)
            .await
            .unwrap();
        let org = database
            .create_organization_with_owner("Staging", "staging", &user.id)
            .await
            .unwrap();
        let project = database.create_project(&org.id, "Staging").await.unwrap();
        let project_id = ProjectId::from(project.id.as_str());
        let metric = NormalizedMetric {
            project_id: Some(project.id.clone()),
            datapoint_id: "same-identity".to_string(),
            content_digest: "new-content".to_string(),
            timestamp: clock.now(),
            ingested_at: Some(clock.now()),
            ..Default::default()
        };
        analytics
            .insert_metrics(std::slice::from_ref(&metric))
            .await
            .unwrap();

        for _ in 0..2 {
            let reference = service
                .stage(
                    &project.id,
                    StagedSignal::Metrics,
                    b"byte-identical-export",
                    vec![StagedRecord::Metric {
                        datapoint_id: metric.datapoint_id.clone(),
                        content_digest: metric.content_digest.clone(),
                        timestamp: metric.timestamp,
                    }],
                    "series".to_string(),
                )
                .await
                .unwrap();
            let (payload, _) = service.load(&reference.id).await.unwrap().unwrap();
            assert_eq!(
                service.settle(&payload).await.unwrap(),
                StagingDisposition::Confirmed
            );
            assert!(
                database
                    .get_staged_payload(&reference.id)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(
                !storage
                    .exists(&project_id, &payload.blob_hash)
                    .await
                    .unwrap()
            );
        }

        let missing = service
            .stage(
                &project.id,
                StagedSignal::Metrics,
                b"missing-export",
                vec![StagedRecord::Metric {
                    datapoint_id: "missing".to_string(),
                    content_digest: "never-written".to_string(),
                    timestamp: clock.now(),
                }],
                String::new(),
            )
            .await
            .unwrap();
        assert!(!service.note_failed_attempt(&missing.id).await.unwrap());
        assert!(service.note_failed_attempt(&missing.id).await.unwrap());
        let held = database
            .get_staged_payload(&missing.id)
            .await
            .unwrap()
            .unwrap();
        assert!(held.unconfirmed);
        assert_eq!(held.redrive_attempts, 2);
        assert!(storage.exists(&project_id, &held.blob_hash).await.unwrap());
        assert!(
            database
                .pending_staged_payloads(10)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
