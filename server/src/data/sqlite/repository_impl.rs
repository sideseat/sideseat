//! TransactionalRepository trait implementation for SQLite
//!
//! This module implements the TransactionalRepository trait for Arc<SqliteService>,
//! providing a unified interface for all transactional database operations.

use std::sync::Arc;

use async_trait::async_trait;

use sideseat_ports::error::DataError;
use sideseat_ports::traits::{
    ApiKeyStore, CredentialStore, DeletionJournal, DeletionRecord, DeletionScope, FavoriteStore,
    FileMetaStore, IdentityStore, ProjectStore,
};
use sideseat_ports::types::{
    ApiKeyRow, ApiKeyScope, ApiKeyValidation, AuthMethodRow, CredentialPermissionRow,
    CredentialRow, FileRow, LastOwnerResult, MemberWithUser, MembershipRow, OrgWithRole,
    OrganizationRow, ProjectRow, UserRow,
};

use super::SqliteService;
use super::repositories::{
    api_key, auth_method, credential_permissions, credentials, favorite, file, journal, membership,
    organization, project, user,
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
        user::create_user(self.0.pool(), None, Some(email), display_name)
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
        user::update_user(self.0.pool(), None, id, display_name)
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
        organization::create_organization_with_owner(self.0.pool(), None, name, slug, owner_user_id)
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
        organization::update_organization(self.0.pool(), None, id, name)
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
        membership::get_membership(self.0.pool(), None, organization_id, user_id)
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
        membership::add_member(self.0.pool(), None, organization_id, user_id, role)
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
        membership::update_role_atomic(self.0.pool(), None, organization_id, user_id, new_role)
            .await
            .map_err(Into::into)
    }

    async fn remove_member_atomic(
        &self,
        organization_id: &str,
        user_id: &str,
    ) -> Result<LastOwnerResult<()>, DataError> {
        membership::remove_member_atomic(self.0.pool(), None, organization_id, user_id)
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
        project::create_project(self.0.pool(), None, organization_id, name)
            .await
            .map_err(Into::into)
    }

    async fn get_project(&self, id: &str) -> Result<Option<ProjectRow>, DataError> {
        project::get_project(self.0.pool(), None, id)
            .await
            .map_err(Into::into)
    }

    async fn update_project(&self, id: &str, name: &str) -> Result<Option<ProjectRow>, DataError> {
        project::update_project(self.0.pool(), None, id, name)
            .await
            .map_err(Into::into)
    }

    async fn list_projects_for_org(
        &self,
        organization_id: &str,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<ProjectRow>, u64), DataError> {
        project::list_for_org(self.0.pool(), None, organization_id, page, limit)
            .await
            .map_err(Into::into)
    }

    async fn list_projects_for_user(
        &self,
        user_id: &str,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<ProjectRow>, u64), DataError> {
        project::list_for_user(self.0.pool(), None, user_id, page, limit)
            .await
            .map_err(Into::into)
    }

    async fn claim_project_for_deletion(&self, id: &str) -> Result<bool, DataError> {
        project::claim_project_for_deletion(self.0.pool(), self.0.cache(), id)
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
        project_id: &str,
        trace_ids: &[String],
    ) -> Result<(), DataError> {
        project::record_deleted_traces(self.0.pool(), project_id, trace_ids)
            .await
            .map_err(Into::into)
    }

    async fn deleted_traces_among(
        &self,
        project_id: &str,
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
        project::get_stale_claimed_projects(self.0.pool(), older_than_secs)
            .await
            .map_err(Into::into)
    }

    async fn reclaim_stale_project(
        &self,
        id: &str,
        observed_deleting_at: i64,
    ) -> Result<bool, DataError> {
        project::reclaim_stale_project(self.0.pool(), id, observed_deleting_at)
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
        project_id: &str,
        session_ids: &[String],
    ) -> Result<(), DataError> {
        project::record_deleted_sessions(self.0.pool(), project_id, session_ids)
            .await
            .map_err(Into::into)
    }

    async fn deleted_sessions_among(
        &self,
        project_id: &str,
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
        project_id: &str,
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
        project_id: &str,
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
        project_id: &str,
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
        project::claim_organization_for_deletion(self.0.pool(), id)
            .await
            .map_err(Into::into)
    }

    async fn get_stale_claimed_organizations(
        &self,
        older_than_secs: i64,
    ) -> Result<Vec<(String, i64)>, DataError> {
        project::get_stale_claimed_organizations(self.0.pool(), older_than_secs)
            .await
            .map_err(Into::into)
    }

    async fn reclaim_stale_organization(
        &self,
        id: &str,
        observed_deleting_at: i64,
    ) -> Result<bool, DataError> {
        project::reclaim_stale_organization(self.0.pool(), id, observed_deleting_at)
            .await
            .map_err(Into::into)
    }

    async fn count_projects_of_organization(&self, org_id: &str) -> Result<i64, DataError> {
        project::count_projects_of_organization(self.0.pool(), org_id)
            .await
            .map_err(Into::into)
    }

    async fn delete_project(&self, id: &str) -> Result<bool, DataError> {
        project::delete_project(self.0.pool(), None, id)
            .await
            .map_err(Into::into)
    }
}

#[async_trait]
impl FileMetaStore for SqliteRepository {
    // ==================== File Operations ====================

    async fn upsert_file(
        &self,
        project_id: &str,
        file_hash: &str,
        media_type: Option<&str>,
        size_bytes: i64,
        hash_algo: &str,
    ) -> Result<i64, DataError> {
        file::upsert_file(
            self.0.pool(),
            project_id,
            file_hash,
            media_type,
            size_bytes,
            hash_algo,
        )
        .await
        .map_err(Into::into)
    }

    async fn get_file(
        &self,
        project_id: &str,
        file_hash: &str,
    ) -> Result<Option<FileRow>, DataError> {
        file::get_file(self.0.pool(), project_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn file_exists(&self, project_id: &str, file_hash: &str) -> Result<bool, DataError> {
        file::file_exists(self.0.pool(), project_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn decrement_ref_count(
        &self,
        project_id: &str,
        file_hash: &str,
    ) -> Result<Option<i64>, DataError> {
        file::decrement_ref_count(self.0.pool(), project_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn delete_file(&self, project_id: &str, file_hash: &str) -> Result<bool, DataError> {
        file::delete_file(self.0.pool(), project_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn delete_project_files(&self, project_id: &str) -> Result<u64, DataError> {
        file::delete_project_files(self.0.pool(), project_id)
            .await
            .map_err(Into::into)
    }

    async fn associate_file(
        &self,
        trace_id: &str,
        project_id: &str,
        file_hash: &str,
        media_type: Option<&str>,
        size_bytes: i64,
        hash_algo: &str,
    ) -> Result<bool, DataError> {
        file::associate_file(
            self.0.pool(),
            trace_id,
            project_id,
            file_hash,
            media_type,
            size_bytes,
            hash_algo,
        )
        .await
        .map_err(Into::into)
    }

    async fn get_file_reference_counts_for_traces(
        &self,
        project_id: &str,
        trace_ids: &[String],
    ) -> Result<Vec<(String, i64)>, DataError> {
        file::get_file_reference_counts_for_traces(self.0.pool(), project_id, trace_ids)
            .await
            .map_err(Into::into)
    }

    async fn associate_existing_file(
        &self,
        trace_id: &str,
        project_id: &str,
        file_hash: &str,
    ) -> Result<bool, DataError> {
        file::associate_existing_file(self.0.pool(), trace_id, project_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn get_stale_claimed_files(
        &self,
        older_than_secs: i64,
    ) -> Result<Vec<(String, String, i64)>, DataError> {
        file::get_stale_claimed_files(self.0.pool(), older_than_secs)
            .await
            .map_err(Into::into)
    }

    async fn reclaim_stale_file(
        &self,
        project_id: &str,
        file_hash: &str,
        observed_deleting_at: i64,
    ) -> Result<bool, DataError> {
        file::reclaim_stale_file(self.0.pool(), project_id, file_hash, observed_deleting_at)
            .await
            .map_err(Into::into)
    }

    async fn claim_file_for_deletion(
        &self,
        project_id: &str,
        file_hash: &str,
    ) -> Result<bool, DataError> {
        file::claim_file_for_deletion(self.0.pool(), project_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn release_deletion_claim(
        &self,
        project_id: &str,
        file_hash: &str,
    ) -> Result<(), DataError> {
        file::release_deletion_claim(self.0.pool(), project_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn restore_orphan_metadata(
        &self,
        project_id: &str,
        file_hash: &str,
        media_type: Option<&str>,
        size_bytes: i64,
        hash_algo: &str,
    ) -> Result<(), DataError> {
        file::restore_orphan_metadata(
            self.0.pool(),
            project_id,
            file_hash,
            media_type,
            size_bytes,
            hash_algo,
        )
        .await
        .map_err(Into::into)
    }

    async fn delete_file_if_unreferenced(
        &self,
        project_id: &str,
        file_hash: &str,
    ) -> Result<bool, DataError> {
        file::delete_file_if_unreferenced(self.0.pool(), project_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn sync_ref_count(
        &self,
        project_id: &str,
        file_hash: &str,
    ) -> Result<Option<i64>, DataError> {
        file::sync_ref_count(self.0.pool(), project_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn insert_trace_file(
        &self,
        trace_id: &str,
        project_id: &str,
        file_hash: &str,
    ) -> Result<(), DataError> {
        file::insert_trace_file(self.0.pool(), trace_id, project_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn get_file_hashes_for_traces(
        &self,
        project_id: &str,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        file::get_file_hashes_for_traces(self.0.pool(), project_id, trace_ids)
            .await
            .map_err(Into::into)
    }

    async fn confirm_trace_file_associations(
        &self,
        associations: &[(String, String, String)],
    ) -> Result<u64, DataError> {
        file::confirm_trace_file_associations(self.0.pool(), associations)
            .await
            .map_err(Into::into)
    }

    async fn release_trace_file_association(
        &self,
        project_id: &str,
        trace_id: &str,
        file_hash: &str,
    ) -> Result<bool, DataError> {
        file::release_trace_file_association(self.0.pool(), project_id, trace_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn delete_trace_files(
        &self,
        project_id: &str,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        file::delete_trace_files(self.0.pool(), project_id, trace_ids)
            .await
            .map_err(Into::into)
    }

    async fn record_retention_cleanup(
        &self,
        project_id: &str,
        trace_ids: &[String],
    ) -> Result<Vec<(String, i64)>, DataError> {
        file::record_retention_cleanup(self.0.pool(), project_id, trace_ids)
            .await
            .map_err(Into::into)
    }

    async fn claim_retention_cleanup(
        &self,
        limit: i64,
        lease_secs: i64,
    ) -> Result<Vec<(String, String, i64)>, DataError> {
        file::claim_retention_cleanup(self.0.pool(), limit, lease_secs)
            .await
            .map_err(Into::into)
    }

    async fn complete_retention_cleanup(
        &self,
        project_id: &str,
        completed: &[(String, i64)],
    ) -> Result<(), DataError> {
        file::complete_retention_cleanup(self.0.pool(), project_id, completed)
            .await
            .map_err(Into::into)
    }

    async fn restore_durable_trace_file(
        &self,
        project_id: &str,
        trace_id: &str,
        file_hash: &str,
    ) -> Result<(), DataError> {
        file::restore_durable_trace_file(self.0.pool(), project_id, trace_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn release_trace_files_except(
        &self,
        project_id: &str,
        trace_id: &str,
        keep: &[String],
    ) -> Result<Vec<String>, DataError> {
        file::release_trace_files_except(self.0.pool(), project_id, trace_id, keep)
            .await
            .map_err(Into::into)
    }

    async fn get_project_storage_bytes(&self, project_id: &str) -> Result<i64, DataError> {
        file::get_project_storage_bytes(self.0.pool(), project_id)
            .await
            .map_err(Into::into)
    }

    async fn get_orphan_files(&self) -> Result<Vec<(String, String)>, DataError> {
        file::get_orphan_files(self.0.pool())
            .await
            .map_err(Into::into)
    }

    async fn get_org_file_storage_bytes(&self, org_id: &str) -> Result<i64, DataError> {
        file::get_org_file_storage_bytes(self.0.pool(), org_id)
            .await
            .map_err(Into::into)
    }

    async fn get_user_file_storage_bytes(&self, user_id: &str) -> Result<i64, DataError> {
        file::get_user_file_storage_bytes(self.0.pool(), user_id)
            .await
            .map_err(Into::into)
    }
}

#[async_trait]
impl ApiKeyStore for SqliteRepository {
    // ==================== API Key Operations ====================

    async fn create_api_key(
        &self,
        org_id: &str,
        name: &str,
        key_hash: &str,
        key_prefix: &str,
        scope: ApiKeyScope,
        created_by: &str,
        expires_at: Option<i64>,
    ) -> Result<ApiKeyRow, DataError> {
        api_key::create_api_key(
            self.0.pool(),
            self.0.cache(),
            org_id,
            name,
            key_hash,
            key_prefix,
            scope,
            created_by,
            expires_at,
        )
        .await
        .map_err(Into::into)
    }

    async fn get_api_key_by_hash(
        &self,
        key_hash: &str,
    ) -> Result<Option<ApiKeyValidation>, DataError> {
        api_key::get_by_hash(self.0.pool(), self.0.cache(), key_hash)
            .await
            .map_err(Into::into)
    }

    async fn list_api_keys(&self, org_id: &str) -> Result<Vec<ApiKeyRow>, DataError> {
        api_key::list_for_org(self.0.pool(), self.0.cache(), org_id)
            .await
            .map_err(Into::into)
    }

    async fn delete_api_key(&self, id: &str, org_id: &str) -> Result<bool, DataError> {
        api_key::delete_api_key(self.0.pool(), self.0.cache(), id, org_id)
            .await
            .map_err(Into::into)
    }

    async fn touch_api_key(&self, id: &str, threshold_secs: u64) -> Result<bool, DataError> {
        api_key::touch_api_key(self.0.pool(), id, threshold_secs)
            .await
            .map_err(Into::into)
    }

    async fn delete_api_keys_for_org(&self, org_id: &str) -> Result<u64, DataError> {
        api_key::delete_for_org(self.0.pool(), self.0.cache(), org_id)
            .await
            .map_err(Into::into)
    }

    async fn get_api_key_hashes_for_org(&self, org_id: &str) -> Result<Vec<String>, DataError> {
        api_key::get_hashes_for_org(self.0.pool(), org_id)
            .await
            .map_err(Into::into)
    }
}

#[async_trait]
impl CredentialStore for SqliteRepository {
    // ==================== Credential Operations ====================

    async fn list_credentials(&self, org_id: &str) -> Result<Vec<CredentialRow>, DataError> {
        credentials::list_credentials(self.0.pool(), org_id)
            .await
            .map_err(Into::into)
    }

    async fn get_credential(
        &self,
        id: &str,
        org_id: &str,
    ) -> Result<Option<CredentialRow>, DataError> {
        credentials::get_credential(self.0.pool(), id, org_id)
            .await
            .map_err(Into::into)
    }

    async fn create_credential(
        &self,
        id: &str,
        org_id: &str,
        provider_key: &str,
        display_name: &str,
        endpoint_url: Option<&str>,
        extra_config: Option<&str>,
        key_preview: Option<&str>,
        created_by: Option<&str>,
    ) -> Result<CredentialRow, DataError> {
        credentials::create_credential(
            self.0.pool(),
            id,
            org_id,
            provider_key,
            display_name,
            endpoint_url,
            extra_config,
            key_preview,
            created_by,
        )
        .await
        .map_err(Into::into)
    }

    async fn update_credential(
        &self,
        id: &str,
        org_id: &str,
        display_name: Option<&str>,
        endpoint_url: Option<Option<&str>>,
        extra_config: Option<Option<&str>>,
    ) -> Result<Option<CredentialRow>, DataError> {
        credentials::update_credential(
            self.0.pool(),
            id,
            org_id,
            display_name,
            endpoint_url,
            extra_config,
        )
        .await
        .map_err(Into::into)
    }

    async fn delete_credential(&self, id: &str, org_id: &str) -> Result<bool, DataError> {
        credentials::delete_credential(self.0.pool(), id, org_id)
            .await
            .map_err(Into::into)
    }

    // ==================== Credential Permission Operations ====================

    async fn list_credential_permissions(
        &self,
        credential_id: &str,
    ) -> Result<Vec<CredentialPermissionRow>, DataError> {
        credential_permissions::list_credential_permissions(self.0.pool(), credential_id)
            .await
            .map_err(Into::into)
    }

    async fn create_credential_permission(
        &self,
        id: &str,
        credential_id: &str,
        org_id: &str,
        project_id: Option<&str>,
        access: &str,
        created_by: Option<&str>,
    ) -> Result<CredentialPermissionRow, DataError> {
        credential_permissions::create_credential_permission(
            self.0.pool(),
            id,
            credential_id,
            org_id,
            project_id,
            access,
            created_by,
        )
        .await
        .map_err(Into::into)
    }

    async fn delete_credential_permission(
        &self,
        id: &str,
        credential_id: &str,
    ) -> Result<bool, DataError> {
        credential_permissions::delete_credential_permission(self.0.pool(), id, credential_id)
            .await
            .map_err(Into::into)
    }

    async fn get_credentials_accessible_by_project(
        &self,
        org_id: &str,
        project_id: &str,
    ) -> Result<Vec<String>, DataError> {
        credential_permissions::get_credentials_accessible_by_project(
            self.0.pool(),
            org_id,
            project_id,
        )
        .await
        .map_err(Into::into)
    }
}

#[async_trait]
impl FavoriteStore for SqliteRepository {
    // ==================== Favorite Operations ====================

    async fn add_favorite(
        &self,
        user_id: &str,
        entity_type: &str,
        entity_id: &str,
        secondary_id: Option<&str>,
        project_id: &str,
    ) -> Result<bool, DataError> {
        favorite::add_favorite(
            self.0.pool(),
            user_id,
            project_id,
            entity_type,
            entity_id,
            secondary_id,
        )
        .await
        .map_err(Into::into)
    }

    async fn remove_favorite(
        &self,
        user_id: &str,
        entity_type: &str,
        entity_id: &str,
        secondary_id: Option<&str>,
        project_id: &str,
    ) -> Result<bool, DataError> {
        favorite::remove_favorite(
            self.0.pool(),
            user_id,
            project_id,
            entity_type,
            entity_id,
            secondary_id,
        )
        .await
        .map_err(Into::into)
    }

    async fn check_favorites(
        &self,
        user_id: &str,
        entity_type: &str,
        entity_ids: &[String],
        project_id: &str,
    ) -> Result<Vec<String>, DataError> {
        let set =
            favorite::check_favorites(self.0.pool(), user_id, project_id, entity_type, entity_ids)
                .await
                .map_err(DataError::from)?;
        Ok(set.into_iter().collect())
    }

    async fn check_span_favorites(
        &self,
        user_id: &str,
        span_ids: &[(String, String)],
        project_id: &str,
    ) -> Result<Vec<(String, String)>, DataError> {
        let set = favorite::check_span_favorites(self.0.pool(), user_id, project_id, span_ids)
            .await
            .map_err(DataError::from)?;
        // Convert "trace_id:span_id" strings back to tuples
        Ok(set
            .into_iter()
            .filter_map(|s| {
                let parts: Vec<&str> = s.splitn(2, ':').collect();
                if parts.len() == 2 {
                    Some((parts[0].to_string(), parts[1].to_string()))
                } else {
                    None
                }
            })
            .collect())
    }

    async fn count_favorites(&self, user_id: &str, project_id: &str) -> Result<i64, DataError> {
        favorite::count_favorites(self.0.pool(), user_id, project_id)
            .await
            .map(|c| c as i64)
            .map_err(Into::into)
    }

    async fn list_favorite_ids(
        &self,
        user_id: &str,
        entity_type: &str,
        project_id: &str,
    ) -> Result<Vec<String>, DataError> {
        // Use a reasonable default limit
        favorite::list_all_favorite_ids(self.0.pool(), user_id, project_id, entity_type, 10000)
            .await
            .map_err(Into::into)
    }

    async fn delete_favorites_by_entity(
        &self,
        entity_type: &str,
        entity_ids: &[String],
        project_id: &str,
    ) -> Result<u64, DataError> {
        favorite::delete_favorites_by_entity(self.0.pool(), project_id, entity_type, entity_ids)
            .await
            .map_err(Into::into)
    }
}

#[async_trait]
impl DeletionJournal for SqliteRepository {
    async fn append_deletions(&self, records: &[DeletionRecord]) -> Result<(), DataError> {
        journal::append_deletions(self.0.pool(), records)
            .await
            .map_err(Into::into)
    }

    async fn deletions_since(
        &self,
        after_sequence: i64,
        limit: usize,
    ) -> Result<(Vec<(i64, DeletionRecord)>, i64), DataError> {
        journal::deletions_since(self.0.pool(), after_sequence, limit)
            .await
            .map_err(Into::into)
    }

    async fn deletion_is_journaled(
        &self,
        project_id: &str,
        scope: DeletionScope,
        target_id: &str,
        span_id: Option<&str>,
    ) -> Result<bool, DataError> {
        journal::deletion_is_journaled(self.0.pool(), project_id, scope, target_id, span_id)
            .await
            .map_err(Into::into)
    }
}
