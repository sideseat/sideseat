//! Project API types

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sideseat_core::utils::collection_fence::FenceStatus;
use utoipa::ToSchema;
use validator::Validate;

use crate::types::{default_limit, default_page, validate_limit, validate_page};
use sideseat_ports::types::{
    ProjectHold, ProjectRow, ProjectStorageUsage, RetentionCleanupBacklog,
};

/// Project DTO for API responses
#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectDto {
    pub id: String,
    pub organization_id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<ProjectRow> for ProjectDto {
    fn from(row: ProjectRow) -> Self {
        Self {
            id: row.id,
            organization_id: row.organization_id,
            name: row.name,
            created_at: DateTime::from_timestamp(row.created_at, 0).unwrap_or(DateTime::UNIX_EPOCH),
            updated_at: DateTime::from_timestamp(row.updated_at, 0).unwrap_or(DateTime::UNIX_EPOCH),
        }
    }
}

/// Request body for creating a project
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateProjectRequest {
    #[validate(length(min = 1, max = 100, message = "Name must be 1-100 characters"))]
    pub name: String,

    /// Organization ID (required - project must belong to an org)
    #[validate(length(
        min = 1,
        max = 100,
        message = "Organization ID must be 1-100 characters"
    ))]
    pub organization_id: String,
}

/// Request body for updating a project
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateProjectRequest {
    #[validate(length(min = 1, max = 100, message = "Name must be 1-100 characters"))]
    pub name: String,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct SetProjectHoldRequest {
    pub hold_until: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectHoldDto {
    pub project_id: String,
    pub hold_until: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<ProjectHold> for ProjectHoldDto {
    fn from(hold: ProjectHold) -> Self {
        Self {
            project_id: hold.project_id.to_string(),
            hold_until: hold.hold_until,
            updated_at: hold.updated_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectStorageUsageDto {
    pub logical_bytes: u64,
    pub quota_bytes: u64,
    pub maintenance_reserve_bytes: u64,
    pub ordinary_write_limit_bytes: u64,
    pub updated_at: DateTime<Utc>,
    pub retention_cleanup: RetentionCleanupDto,
    pub collection: CollectionStatusDto,
}

/// Whether the serving instance deletes and collects. It stops while it cannot renew its lease on the
/// transactional store, and once it holds the lease again it waits for `waiting_for` - the passes begun before the
/// loss - before it resumes. A long-running pass there is collection stalled, not storage growing.
#[derive(Debug, Serialize, ToSchema)]
pub struct CollectionStatusDto {
    pub collecting: bool,
    pub waiting_for: Vec<RunningPassDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RunningPassDto {
    pub pass: String,
    pub running_secs: u64,
}

impl From<FenceStatus> for CollectionStatusDto {
    fn from(status: FenceStatus) -> Self {
        Self {
            collecting: status.open,
            // While collecting, the passes running are ordinary work, not what anything waits for.
            waiting_for: if status.open {
                Vec::new()
            } else {
                status
                    .running
                    .into_iter()
                    .map(|pass| RunningPassDto {
                        pass: pass.name.to_string(),
                        running_secs: pass.running_for.as_secs(),
                    })
                    .collect()
            },
        }
    }
}

/// Retention cleanups still owed: each releases the files, favourites and raw-record ownership of one trace
/// retention removed, and is retried until it completes. An old `oldest_pending_at` with a growing
/// `most_attempts` is retention stalled - a ClickHouse replica unreachable, say - rather than storage growing.
#[derive(Debug, Serialize, ToSchema)]
pub struct RetentionCleanupDto {
    pub pending: u64,
    pub oldest_pending_at: Option<DateTime<Utc>>,
    pub oldest_pending_secs: Option<i64>,
    pub most_attempts: u64,
}

impl RetentionCleanupDto {
    pub fn new(backlog: RetentionCleanupBacklog, now: DateTime<Utc>) -> Self {
        Self {
            pending: backlog.pending,
            oldest_pending_at: backlog.oldest_pending_at,
            oldest_pending_secs: backlog
                .oldest_pending_at
                .map(|at| (now - at).num_seconds().max(0)),
            most_attempts: backlog.most_attempts,
        }
    }
}

impl ProjectStorageUsageDto {
    pub fn new(
        usage: ProjectStorageUsage,
        quota_bytes: u64,
        maintenance_reserve_bytes: u64,
        ordinary_write_limit_bytes: u64,
        retention_cleanup: RetentionCleanupDto,
        collection: CollectionStatusDto,
    ) -> Self {
        Self {
            logical_bytes: usage.logical_bytes,
            quota_bytes,
            maintenance_reserve_bytes,
            ordinary_write_limit_bytes,
            updated_at: usage.updated_at,
            retention_cleanup,
            collection,
        }
    }
}

/// Query params for listing projects
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ListProjectsQuery {
    #[serde(default = "default_page")]
    #[validate(custom(function = "validate_page"))]
    pub page: u32,

    #[serde(default = "default_limit")]
    #[validate(custom(function = "validate_limit"))]
    pub limit: u32,

    /// Optional organization ID filter
    pub org_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sideseat_core::utils::collection_fence::RunningPass;
    use std::time::Duration;

    /// Stopped, the status names what collection waits for and how long it has run; collecting, it names none.
    #[test]
    fn collection_status_names_what_a_stopped_collection_waits_for() {
        let running = vec![RunningPass {
            name: "retention",
            running_for: Duration::from_secs(95),
        }];
        let stalled = CollectionStatusDto::from(FenceStatus {
            open: false,
            running: running.clone(),
        });
        assert!(!stalled.collecting);
        assert_eq!(stalled.waiting_for.len(), 1);
        assert_eq!(stalled.waiting_for[0].pass, "retention");
        assert_eq!(stalled.waiting_for[0].running_secs, 95);
        let collecting = CollectionStatusDto::from(FenceStatus {
            open: true,
            running,
        });
        assert!(collecting.collecting);
        assert!(collecting.waiting_for.is_empty());
    }
}
