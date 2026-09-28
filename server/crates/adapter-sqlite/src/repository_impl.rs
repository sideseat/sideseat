//! Transactional repository port implementation for SQLite.
//!
//! This module implements the TransactionalRepository trait for Arc<SqliteService>,
//! providing a unified interface for all transactional database operations.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use sideseat_ports::error::DataError;
use sideseat_ports::traits::{
    ApiKeyStore, ContentBodyStore, CredentialStore, DeletionJournal, DeletionRecord, DeletionScope,
    FavoriteStore, FileMetaStore, IdentityStore, ProjectStore, StagedPayloadStore,
    StorageGovernance,
};
use sideseat_ports::types::{
    ApiKeyRow, ApiKeyScope, ApiKeyValidation, AuthMethodRow, ContentBodyBackfillProgress,
    ContentBodyObject, CredentialPermissionRow, CredentialRow, FileRow, LastOwnerResult,
    MemberWithUser, MembershipRow, OrgWithRole, OrganizationRow, ProjectHold, ProjectId,
    ProjectRow, ProjectStorageUsage, SpanBodyAssociation, SpanBodyField, StagedPayload, UserRow,
};

use super::SqliteService;
use super::repositories::{
    api_key, auth_method, body, credential_permissions, credentials, favorite, file, governance,
    journal, membership, organization, project, restore, staging, user,
};

/// The port, implemented over the service.
///
/// A **wrapper rather than `impl … for Arc<SqliteService>`**, and that is the orphan rule rather than taste: with the
/// trait in `sideseat-ports` and `Arc` in `std`, an impl on `Arc<SqliteService>` has no local type ahead of an
/// uncovered parameter, so it is refused across a crate boundary. It compiled only while everything was one
/// crate - which is one more way the single crate hid the direction of its own dependencies.
#[derive(Clone)]
pub struct SqliteRepository(pub Arc<SqliteService>);

/// So the wrapper is transparent to the service's own methods.
///
/// Without this, wrapping turns every call that is *not* a port method - a maintenance helper, a test probe -
/// into `wrapper.0.method()`, which is noise that says nothing. The port methods live on the wrapper itself and
/// are found first, so nothing is shadowed.
impl std::ops::Deref for SqliteRepository {
    type Target = SqliteService;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[async_trait]
impl IdentityStore for SqliteRepository {
    // ==================== User Operations ====================

    async fn create_user(
        &self,
        email: &str,
        display_name: Option<&str>,
    ) -> Result<UserRow, DataError> {
        user::create_user(
            self.0.pool(),
            None,
            Some(email),
            display_name,
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn get_user(&self, id: &str) -> Result<Option<UserRow>, DataError> {
        user::get_user(self.0.pool(), None, id)
            .await
            .map_err(Into::into)
    }

    async fn get_user_by_email(&self, email: &str) -> Result<Option<UserRow>, DataError> {
        user::get_by_email(self.0.pool(), None, email)
            .await
            .map_err(Into::into)
    }

    async fn update_user(
        &self,
        id: &str,
        display_name: Option<&str>,
    ) -> Result<Option<UserRow>, DataError> {
        user::update_user(
            self.0.pool(),
            None,
            id,
            display_name,
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    // ==================== Organization Operations ====================

    async fn create_organization_with_owner(
        &self,
        name: &str,
        slug: &str,
        owner_user_id: &str,
    ) -> Result<OrganizationRow, DataError> {
        organization::create_organization_with_owner(
            self.0.pool(),
            None,
            name,
            slug,
            owner_user_id,
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn get_organization(&self, id: &str) -> Result<Option<OrganizationRow>, DataError> {
        organization::get_organization(self.0.pool(), None, id)
            .await
            .map_err(Into::into)
    }

    async fn update_organization(
        &self,
        id: &str,
        name: &str,
    ) -> Result<Option<OrganizationRow>, DataError> {
        organization::update_organization(
            self.0.pool(),
            None,
            id,
            name,
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn list_orgs_for_user(
        &self,
        user_id: &str,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<OrgWithRole>, u64), DataError> {
        organization::list_for_user(self.0.pool(), None, user_id, page, limit)
            .await
            .map_err(Into::into)
    }

    async fn delete_organization(&self, id: &str) -> Result<bool, DataError> {
        organization::delete_organization(self.0.pool(), None, id)
            .await
            .map_err(Into::into)
    }

    async fn list_project_ids(&self, organization_id: &str) -> Result<Vec<String>, DataError> {
        organization::list_project_ids(self.0.pool(), organization_id)
            .await
            .map_err(Into::into)
    }

    // ==================== Membership Operations ====================

    async fn get_membership(
        &self,
        organization_id: &str,
        user_id: &str,
    ) -> Result<Option<MembershipRow>, DataError> {
        membership::get_membership(self.0.pool(), organization_id, user_id)
            .await
            .map_err(Into::into)
    }

    async fn get_member_with_user(
        &self,
        organization_id: &str,
        user_id: &str,
    ) -> Result<Option<MemberWithUser>, DataError> {
        membership::get_member_with_user(self.0.pool(), organization_id, user_id)
            .await
            .map_err(Into::into)
    }

    async fn add_member(
        &self,
        organization_id: &str,
        user_id: &str,
        role: &str,
    ) -> Result<MembershipRow, DataError> {
        membership::add_member(
            self.0.pool(),
            organization_id,
            user_id,
            role,
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn list_members(
        &self,
        organization_id: &str,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<MemberWithUser>, u64), DataError> {
        membership::list_members(self.0.pool(), organization_id, page, limit)
            .await
            .map_err(Into::into)
    }

    async fn update_role_atomic(
        &self,
        organization_id: &str,
        user_id: &str,
        new_role: &str,
    ) -> Result<LastOwnerResult<MembershipRow>, DataError> {
        membership::update_role_atomic(
            self.0.pool(),
            organization_id,
            user_id,
            new_role,
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn remove_member_atomic(
        &self,
        organization_id: &str,
        user_id: &str,
    ) -> Result<LastOwnerResult<()>, DataError> {
        membership::remove_member_atomic(self.0.pool(), organization_id, user_id)
            .await
            .map_err(Into::into)
    }

    // ==================== Auth Method Operations ====================

    #[allow(clippy::too_many_arguments)]
    async fn create_auth_method(
        &self,
        user_id: &str,
        method_type: &str,
        provider: Option<&str>,
        provider_id: Option<&str>,
        credential_hash: Option<&str>,
        metadata: Option<&str>,
    ) -> Result<AuthMethodRow, DataError> {
        auth_method::create_auth_method(
            self.0.pool(),
            None,
            user_id,
            method_type,
            provider,
            provider_id,
            credential_hash,
            metadata,
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn find_auth_by_oauth(
        &self,
        provider: &str,
        provider_id: &str,
    ) -> Result<Option<AuthMethodRow>, DataError> {
        auth_method::find_by_oauth(self.0.pool(), None, provider, provider_id)
            .await
            .map_err(Into::into)
    }

    async fn list_auth_methods_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<AuthMethodRow>, DataError> {
        auth_method::list_for_user(self.0.pool(), None, user_id)
            .await
            .map_err(Into::into)
    }

    async fn delete_auth_method(&self, id: &str) -> Result<bool, DataError> {
        auth_method::delete_auth_method(self.0.pool(), None, id)
            .await
            .map_err(Into::into)
    }

    async fn get_bootstrap_method(
        &self,
        user_id: &str,
    ) -> Result<Option<AuthMethodRow>, DataError> {
        auth_method::get_bootstrap_method(self.0.pool(), user_id)
            .await
            .map_err(Into::into)
    }
}

#[async_trait]
impl ProjectStore for SqliteRepository {
    // ==================== Project Operations ====================

    async fn create_project(
        &self,
        organization_id: &str,
        name: &str,
    ) -> Result<ProjectRow, DataError> {
        project::create_project(
            self.0.pool(),
            organization_id,
            name,
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn get_project(&self, id: &str) -> Result<Option<ProjectRow>, DataError> {
        project::get_project(self.0.pool(), id)
            .await
            .map_err(Into::into)
    }

    async fn update_project(&self, id: &str, name: &str) -> Result<Option<ProjectRow>, DataError> {
        project::update_project(self.0.pool(), id, name, self.0.clock().now().timestamp())
            .await
            .map_err(Into::into)
    }

    async fn list_projects_for_org(
        &self,
        organization_id: &str,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<ProjectRow>, u64), DataError> {
        project::list_for_org(self.0.pool(), organization_id, page, limit)
            .await
            .map_err(Into::into)
    }

    async fn list_projects_for_user(
        &self,
        user_id: &str,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<ProjectRow>, u64), DataError> {
        project::list_for_user(self.0.pool(), user_id, page, limit)
            .await
            .map_err(Into::into)
    }

    async fn list_projects(
        &self,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<ProjectRow>, u64), DataError> {
        project::list_projects(self.0.pool(), page, limit)
            .await
            .map_err(Into::into)
    }

    async fn restore_project_ids(&self, limit: usize) -> Result<Vec<ProjectId>, DataError> {
        project::restore_project_ids(self.0.pool(), limit)
            .await
            .map_err(Into::into)
    }

    async fn claim_project_for_deletion(&self, id: &str) -> Result<bool, DataError> {
        project::claim_project_for_deletion(self.0.pool(), id, self.0.clock().now().timestamp())
            .await
            .map_err(Into::into)
    }

    async fn project_accepts_writes(&self, id: &str) -> Result<bool, DataError> {
        project::project_accepts_writes(self.0.pool(), id)
            .await
            .map_err(Into::into)
    }

    async fn record_deleted_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<(), DataError> {
        project::record_deleted_traces(
            self.0.pool(),
            project_id,
            trace_ids,
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn deleted_traces_among(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<std::collections::HashSet<String>, DataError> {
        project::deleted_traces_among(self.0.pool(), project_id, trace_ids)
            .await
            .map_err(Into::into)
    }

    async fn get_stale_claimed_projects(
        &self,
        older_than_secs: i64,
    ) -> Result<Vec<(String, i64)>, DataError> {
        project::get_stale_claimed_projects(
            self.0.pool(),
            older_than_secs,
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn reclaim_stale_project(
        &self,
        id: &ProjectId,
        observed_deleting_at: i64,
    ) -> Result<bool, DataError> {
        project::reclaim_stale_project(
            self.0.pool(),
            id,
            observed_deleting_at,
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn record_project_sweep(
        &self,
        id: &str,
        was_clean: bool,
        required: i64,
        min_gap_secs: i64,
    ) -> Result<bool, DataError> {
        project::record_project_sweep(self.0.pool(), id, was_clean, required, min_gap_secs)
            .await
            .map_err(Into::into)
    }

    async fn claim_deleted_projects_for_check(
        &self,
        lease_secs: i64,
        limit: i64,
    ) -> Result<Vec<(String, i64)>, DataError> {
        project::claim_deleted_projects_for_check(self.0.pool(), lease_secs, limit)
            .await
            .map_err(Into::into)
    }

    async fn record_deleted_sessions(
        &self,
        project_id: &ProjectId,
        session_ids: &[String],
    ) -> Result<(), DataError> {
        project::record_deleted_sessions(
            self.0.pool(),
            project_id,
            session_ids,
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn deleted_sessions_among(
        &self,
        project_id: &ProjectId,
        session_ids: &[String],
    ) -> Result<std::collections::HashSet<String>, DataError> {
        project::deleted_sessions_among(self.0.pool(), project_id, session_ids)
            .await
            .map_err(Into::into)
    }

    async fn claim_deleted_sessions_for_check(
        &self,
        lease_secs: i64,
        limit: i64,
    ) -> Result<Vec<(String, String, i64)>, DataError> {
        project::claim_deleted_sessions_for_check(self.0.pool(), lease_secs, limit)
            .await
            .map_err(Into::into)
    }

    async fn record_deleted_session_check(
        &self,
        project_id: &ProjectId,
        session_id: &str,
        claim_token: i64,
        was_quiet: bool,
        base_gap_secs: i64,
        max_gap_secs: i64,
    ) -> Result<(), DataError> {
        project::record_deleted_session_check(
            self.0.pool(),
            project_id,
            session_id,
            claim_token,
            was_quiet,
            base_gap_secs,
            max_gap_secs,
        )
        .await
        .map_err(Into::into)
    }

    async fn claim_deleted_traces_for_check(
        &self,
        lease_secs: i64,
        limit: i64,
    ) -> Result<Vec<(String, String, i64)>, DataError> {
        project::claim_deleted_traces_for_check(self.0.pool(), lease_secs, limit)
            .await
            .map_err(Into::into)
    }

    async fn record_deleted_trace_check(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        claim_token: i64,
        was_quiet: bool,
        base_gap_secs: i64,
        max_gap_secs: i64,
    ) -> Result<(), DataError> {
        project::record_deleted_trace_check(
            self.0.pool(),
            project_id,
            trace_id,
            claim_token,
            was_quiet,
            base_gap_secs,
            max_gap_secs,
        )
        .await
        .map_err(Into::into)
    }

    async fn record_deleted_project_check(
        &self,
        project_id: &ProjectId,
        claim_token: i64,
        was_quiet: bool,
        base_gap_secs: i64,
        max_gap_secs: i64,
    ) -> Result<(), DataError> {
        project::record_deleted_project_check(
            self.0.pool(),
            project_id,
            claim_token,
            was_quiet,
            base_gap_secs,
            max_gap_secs,
        )
        .await
        .map_err(Into::into)
    }

    async fn forget_deleted_projects(&self, retention_secs: i64) -> Result<u64, DataError> {
        project::forget_deleted_projects(self.0.pool(), retention_secs)
            .await
            .map_err(Into::into)
    }

    async fn claim_organization_for_deletion(&self, id: &str) -> Result<bool, DataError> {
        project::claim_organization_for_deletion(
            self.0.pool(),
            id,
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn get_stale_claimed_organizations(
        &self,
        older_than_secs: i64,
    ) -> Result<Vec<(String, i64)>, DataError> {
        project::get_stale_claimed_organizations(
            self.0.pool(),
            older_than_secs,
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn reclaim_stale_organization(
        &self,
        id: &str,
        observed_deleting_at: i64,
    ) -> Result<bool, DataError> {
        project::reclaim_stale_organization(
            self.0.pool(),
            id,
            observed_deleting_at,
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn count_projects_of_organization(&self, org_id: &str) -> Result<i64, DataError> {
        project::count_projects_of_organization(self.0.pool(), org_id)
            .await
            .map_err(Into::into)
    }

    async fn delete_project(&self, id: &str) -> Result<bool, DataError> {
        project::delete_project(self.0.pool(), id)
            .await
            .map_err(Into::into)
    }
}

mod files;
mod settings;
