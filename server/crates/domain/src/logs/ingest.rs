//! Fence, validate and persist OTLP logs before acknowledgement.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use sideseat_core::constants::DEFAULT_PROJECT_ID;
use sideseat_core::utils::retry::{
    DEFAULT_BASE_DELAY_MS, DEFAULT_MAX_ATTEMPTS, retry_with_backoff_async,
};
use sideseat_core::utils::time::is_storable;
use sideseat_ports::traits::{AnalyticsRepository, TransactionalRepository};
use sideseat_ports::types::ProjectId;

use super::extract::extract_logs_batch;
use crate::storage_governance::StorageGovernanceService;

pub struct Stored {
    pub stored: usize,
    pub total: usize,
    pub gone: usize,
    pub unstorable: usize,
}

pub async fn ingest(
    request: &ExportLogsServiceRequest,
    analytics: &(dyn AnalyticsRepository + Send + Sync),
    database: &(dyn TransactionalRepository + Send + Sync),
    received_at: DateTime<Utc>,
) -> Result<Stored, String> {
    ingest_inner(request, analytics, database, received_at, None).await
}

pub async fn ingest_governed(
    request: &ExportLogsServiceRequest,
    analytics: &(dyn AnalyticsRepository + Send + Sync),
    database: &(dyn TransactionalRepository + Send + Sync),
    received_at: DateTime<Utc>,
    governance: &StorageGovernanceService,
) -> Result<Stored, String> {
    ingest_inner(request, analytics, database, received_at, Some(governance)).await
}

async fn ingest_inner(
    request: &ExportLogsServiceRequest,
    analytics: &(dyn AnalyticsRepository + Send + Sync),
    database: &(dyn TransactionalRepository + Send + Sync),
    received_at: DateTime<Utc>,
    governance: Option<&StorageGovernanceService>,
) -> Result<Stored, String> {
    let mut logs = extract_logs_batch(request, received_at);
    let total = logs.len();
    let mut gone = 0;

    let mut projects: Vec<&str> = logs
        .iter()
        .map(|log| log.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID))
        .collect();
    projects.sort_unstable();
    projects.dedup();
    let mut refusing = HashSet::new();
    for project in projects {
        match database.project_accepts_writes(project).await {
            Ok(true) => {}
            Ok(false) => {
                refusing.insert(project.to_string());
            }
            Err(error) => return Err(format!("could not read the project fence: {error}")),
        }
    }
    if !refusing.is_empty() {
        let before = logs.len();
        logs.retain(|log| {
            !refusing.contains(log.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID))
        });
        gone = before - logs.len();
    }

    let before = logs.len();
    logs.retain(|log| {
        is_storable(log.timestamp)
            && log.time.map(is_storable).unwrap_or(true)
            && log.observed_time.map(is_storable).unwrap_or(true)
    });
    let unstorable = before - logs.len();

    if !logs.is_empty() {
        crate::search::index_logs(&mut logs);
        if let Some(governance) = governance {
            governance
                .stamp_logs(&mut logs)
                .await
                .map_err(|error| error.to_string())?;
        }
        let written_projects = logs
            .iter()
            .map(|log| ProjectId::from(log.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID)))
            .collect::<Vec<_>>();
        retry_with_backoff_async(DEFAULT_MAX_ATTEMPTS, DEFAULT_BASE_DELAY_MS, || {
            analytics.insert_logs(&logs)
        })
        .await
        .map_err(|(error, _)| error.to_string())?;
        if let Some(governance) = governance {
            governance
                .patch_after_write(&written_projects)
                .await
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(Stored {
        stored: logs.len(),
        total,
        gone,
        unstorable,
    })
}
