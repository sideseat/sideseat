//! Cross-database cleanup logic for organization and project deletion

use std::sync::Arc;

use anyhow::{Context, Result, anyhow};

use crate::data::AnalyticsService;
use crate::data::TransactionalService;
use crate::data::cache::{CacheKey, CacheService};
use crate::data::files::FileService;

/// Cleanup for organization deletion
///
/// Performs cleanup in the correct order:
/// 1. Get project_ids before transactional cascade deletes them
/// 2. Delete traces from analytics backend for each project
/// 3. Delete files from filesystem for each project
/// 4. Invalidate API key caches for the organization
/// 5. Delete org from transactional backend (cascades to projects, members, api_keys)
///
/// All cleanup steps are attempted even if some fail. Errors are collected
/// and returned after all steps complete to ensure maximum cleanup.
pub async fn cleanup_organization(
    database: &Arc<TransactionalService>,
    analytics: &Arc<AnalyticsService>,
    file_service: &Arc<FileService>,
    cache: Option<&CacheService>,
    org_id: &str,
) -> Result<bool> {
    let mut errors: Vec<String> = Vec::new();

    let repo = database.repository();
    let analytics_repo = analytics.repository();

    // 1. Get project_ids before transactional cascade deletes them
    let project_ids = repo.list_project_ids(org_id).await?;

    // 2. Delete traces from analytics backend for each project
    for project_id in &project_ids {
        if let Err(e) = analytics_repo.delete_project_data(project_id).await {
            errors.push(format!(
                "Analytics delete failed for project {}: {}",
                project_id, e
            ));
        }
    }

    // 3. Delete files from filesystem for each project
    for project_id in &project_ids {
        if let Err(e) = file_service.delete_project(project_id).await {
            errors.push(format!(
                "File delete failed for project {}: {}",
                project_id, e
            ));
        }
    }

    // 4. Invalidate API key caches for the organization
    if let Some(cache) = cache {
        invalidate_org_api_key_caches(repo.as_ref(), cache, org_id).await;
    }

    // 5. Delete org from transactional backend (cascades to projects, members, api_keys, files metadata)
    let deleted = repo
        .delete_organization(None, org_id)
        .await
        .context("Failed to delete organization")?;

    // Return error if any cleanup step failed
    if !errors.is_empty() {
        return Err(anyhow!(
            "Organization {} cleanup completed with {} errors: {}",
            org_id,
            errors.len(),
            errors.join("; ")
        ));
    }

    Ok(deleted)
}

/// Cleanup for single project deletion
///
/// Performs cleanup in the correct order:
/// 1. Delete traces from analytics backend
/// 2. Delete files from filesystem
/// 3. Delete project from transactional backend
///
/// Note: API keys are org-scoped, not project-scoped, so no API key cleanup is needed here.
///
/// All cleanup steps are attempted even if some fail. Errors are collected
/// and returned after all steps complete to ensure maximum cleanup.
pub async fn cleanup_project(
    database: &Arc<TransactionalService>,
    analytics: &Arc<AnalyticsService>,
    file_service: &Arc<FileService>,
    _cache: Option<&CacheService>,
    project_id: &str,
) -> Result<bool> {
    let mut errors: Vec<String> = Vec::new();

    let repo = database.repository();
    let analytics_repo = analytics.repository();

    // 1. Delete traces from analytics backend
    if let Err(e) = analytics_repo.delete_project_data(project_id).await {
        errors.push(format!("Analytics delete failed: {}", e));
    }

    // 2. Delete files from filesystem
    if let Err(e) = file_service.delete_project(project_id).await {
        errors.push(format!("File delete failed: {}", e));
    }

    // Note: API keys are org-scoped, no cleanup needed here

    // 3. Delete project from transactional backend (cascades to file metadata)
    let deleted = repo
        .delete_project(None, project_id)
        .await
        .context("Failed to delete project")?;

    // Return error if any cleanup step failed
    if !errors.is_empty() {
        return Err(anyhow!(
            "Project {} cleanup completed with {} errors: {}",
            project_id,
            errors.len(),
            errors.join("; ")
        ));
    }

    Ok(deleted)
}

/// Invalidate API key caches for an organization
///
/// Fetches all API key hashes and invalidates their individual caches,
/// then invalidates the organization's API key list cache.
async fn invalidate_org_api_key_caches(
    repo: &dyn crate::data::traits::TransactionalRepository,
    cache: &CacheService,
    org_id: &str,
) {
    // Get all key hashes for this organization
    match repo.get_api_key_hashes_for_org(org_id).await {
        Ok(hashes) => {
            // Invalidate individual key caches
            for hash in hashes {
                let key = CacheKey::api_key_by_hash(&hash);
                if let Err(e) = cache.delete(&key).await {
                    tracing::debug!(
                        org_id = %org_id,
                        error = %e,
                        "Failed to invalidate API key cache"
                    );
                }
            }
        }
        Err(e) => {
            tracing::debug!(
                org_id = %org_id,
                error = %e,
                "Failed to get API key hashes for cache invalidation"
            );
        }
    }

    // Invalidate organization's API key list cache
    let list_key = CacheKey::api_keys_for_org(org_id);
    if let Err(e) = cache.delete(&list_key).await {
        tracing::debug!(
            org_id = %org_id,
            error = %e,
            "Failed to invalidate API key list cache"
        );
    }
}
