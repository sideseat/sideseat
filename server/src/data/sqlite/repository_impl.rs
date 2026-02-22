//! TransactionalRepository trait implementation for SQLite
//!
//! This module implements the TransactionalRepository trait for Arc<SqliteService>,
//! providing a unified interface for all transactional database operations.

use std::sync::Arc;

use async_trait::async_trait;

use crate::data::cache::CacheService;
use crate::data::error::DataError;
use crate::data::traits::TransactionalRepository;
use crate::data::types::{
    ApiKeyRow, ApiKeyScope, ApiKeyValidation, AuthMethodRow, FileRow, LastOwnerResult,
    MemberWithUser, MembershipRow, OrgWithRole, OrganizationRow, ProjectRow, UserRow,
};

use super::SqliteService;
use super::repositories::{
    api_key, auth_method, favorite, file, membership, organization, project, user,
};

#[async_trait]
impl TransactionalRepository for Arc<SqliteService> {
    // ==================== User Operations ====================

    async fn create_user(
        &self,
        cache: Option<&CacheService>,
        email: &str,
        display_name: Option<&str>,
    ) -> Result<UserRow, DataError> {
        user::create_user(self.pool(), cache, Some(email), display_name)
            .await
            .map_err(Into::into)
    }

    async fn get_user(
        &self,
        cache: Option<&CacheService>,
        id: &str,
    ) -> Result<Option<UserRow>, DataError> {
        user::get_user(self.pool(), cache, id)
            .await
            .map_err(Into::into)
    }

    async fn get_user_by_email(
        &self,
        cache: Option<&CacheService>,
        email: &str,
    ) -> Result<Option<UserRow>, DataError> {
        user::get_by_email(self.pool(), cache, email)
            .await
            .map_err(Into::into)
    }

    async fn update_user(
        &self,
        cache: Option<&CacheService>,
        id: &str,
        display_name: Option<&str>,
    ) -> Result<Option<UserRow>, DataError> {
        user::update_user(self.pool(), cache, id, display_name)
            .await
            .map_err(Into::into)
    }

    // ==================== Organization Operations ====================

    async fn create_organization_with_owner(
        &self,
        cache: Option<&CacheService>,
        name: &str,
        slug: &str,
        owner_user_id: &str,
    ) -> Result<OrganizationRow, DataError> {
        organization::create_organization_with_owner(self.pool(), cache, name, slug, owner_user_id)
            .await
            .map_err(Into::into)
    }

    async fn get_organization(
        &self,
        cache: Option<&CacheService>,
        id: &str,
    ) -> Result<Option<OrganizationRow>, DataError> {
        organization::get_organization(self.pool(), cache, id)
            .await
            .map_err(Into::into)
    }

    async fn update_organization(
        &self,
        cache: Option<&CacheService>,
        id: &str,
        name: &str,
    ) -> Result<Option<OrganizationRow>, DataError> {
        organization::update_organization(self.pool(), cache, id, name)
            .await
            .map_err(Into::into)
    }

    async fn list_orgs_for_user(
        &self,
        cache: Option<&CacheService>,
        user_id: &str,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<OrgWithRole>, u64), DataError> {
        organization::list_for_user(self.pool(), cache, user_id, page, limit)
            .await
            .map_err(Into::into)
    }

    async fn delete_organization(
        &self,
        cache: Option<&CacheService>,
        id: &str,
    ) -> Result<bool, DataError> {
        organization::delete_organization(self.pool(), cache, id)
            .await
            .map_err(Into::into)
    }

    async fn list_project_ids(&self, organization_id: &str) -> Result<Vec<String>, DataError> {
        organization::list_project_ids(self.pool(), organization_id)
            .await
            .map_err(Into::into)
    }

    // ==================== Membership Operations ====================

    async fn get_membership(
        &self,
        cache: Option<&CacheService>,
        organization_id: &str,
        user_id: &str,
    ) -> Result<Option<MembershipRow>, DataError> {
        membership::get_membership(self.pool(), cache, organization_id, user_id)
            .await
            .map_err(Into::into)
    }

    async fn get_member_with_user(
        &self,
        organization_id: &str,
        user_id: &str,
    ) -> Result<Option<MemberWithUser>, DataError> {
        membership::get_member_with_user(self.pool(), organization_id, user_id)
            .await
            .map_err(Into::into)
    }

    async fn add_member(
        &self,
        cache: Option<&CacheService>,
        organization_id: &str,
        user_id: &str,
        role: &str,
    ) -> Result<MembershipRow, DataError> {
        membership::add_member(self.pool(), cache, organization_id, user_id, role)
            .await
            .map_err(Into::into)
    }

    async fn list_members(
        &self,
        organization_id: &str,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<MemberWithUser>, u64), DataError> {
        membership::list_members(self.pool(), organization_id, page, limit)
            .await
            .map_err(Into::into)
    }

    async fn update_role_atomic(
        &self,
        cache: Option<&CacheService>,
        organization_id: &str,
        user_id: &str,
        new_role: &str,
    ) -> Result<LastOwnerResult<MembershipRow>, DataError> {
        membership::update_role_atomic(self.pool(), cache, organization_id, user_id, new_role)
            .await
            .map_err(Into::into)
    }

    async fn remove_member_atomic(
        &self,
        cache: Option<&CacheService>,
        organization_id: &str,
        user_id: &str,
    ) -> Result<LastOwnerResult<()>, DataError> {
        membership::remove_member_atomic(self.pool(), cache, organization_id, user_id)
            .await
            .map_err(Into::into)
    }

    // ==================== Project Operations ====================

    async fn create_project(
        &self,
        cache: Option<&CacheService>,
        organization_id: &str,
        name: &str,
    ) -> Result<ProjectRow, DataError> {
        project::create_project(self.pool(), cache, organization_id, name)
            .await
            .map_err(Into::into)
    }

    async fn get_project(
        &self,
        cache: Option<&CacheService>,
        id: &str,
    ) -> Result<Option<ProjectRow>, DataError> {
        project::get_project(self.pool(), cache, id)
            .await
            .map_err(Into::into)
    }

    async fn update_project(
        &self,
        cache: Option<&CacheService>,
        id: &str,
        name: &str,
    ) -> Result<Option<ProjectRow>, DataError> {
        project::update_project(self.pool(), cache, id, name)
            .await
            .map_err(Into::into)
    }

    async fn list_projects_for_org(
        &self,
        cache: Option<&CacheService>,
        organization_id: &str,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<ProjectRow>, u64), DataError> {
        project::list_for_org(self.pool(), cache, organization_id, page, limit)
            .await
            .map_err(Into::into)
    }

    async fn list_projects_for_user(
        &self,
        cache: Option<&CacheService>,
        user_id: &str,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<ProjectRow>, u64), DataError> {
        project::list_for_user(self.pool(), cache, user_id, page, limit)
            .await
            .map_err(Into::into)
    }

    async fn delete_project(
        &self,
        cache: Option<&CacheService>,
        id: &str,
    ) -> Result<bool, DataError> {
        project::delete_project(self.pool(), cache, id)
            .await
            .map_err(Into::into)
    }

    // ==================== Auth Method Operations ====================

    #[allow(clippy::too_many_arguments)]
    async fn create_auth_method(
        &self,
        cache: Option<&CacheService>,
        user_id: &str,
        method_type: &str,
        provider: Option<&str>,
        provider_id: Option<&str>,
        credential_hash: Option<&str>,
        metadata: Option<&str>,
    ) -> Result<AuthMethodRow, DataError> {
        auth_method::create_auth_method(
            self.pool(),
            cache,
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
        cache: Option<&CacheService>,
        provider: &str,
        provider_id: &str,
    ) -> Result<Option<AuthMethodRow>, DataError> {
        auth_method::find_by_oauth(self.pool(), cache, provider, provider_id)
            .await
            .map_err(Into::into)
    }

    async fn list_auth_methods_for_user(
        &self,
        cache: Option<&CacheService>,
        user_id: &str,
    ) -> Result<Vec<AuthMethodRow>, DataError> {
        auth_method::list_for_user(self.pool(), cache, user_id)
            .await
            .map_err(Into::into)
    }

    async fn delete_auth_method(
        &self,
        cache: Option<&CacheService>,
        id: &str,
    ) -> Result<bool, DataError> {
        auth_method::delete_auth_method(self.pool(), cache, id)
            .await
            .map_err(Into::into)
    }

    async fn get_bootstrap_method(
        &self,
        user_id: &str,
    ) -> Result<Option<AuthMethodRow>, DataError> {
        auth_method::get_bootstrap_method(self.pool(), user_id)
            .await
            .map_err(Into::into)
    }

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
            self.pool(),
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
            self.pool(),
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
            favorite::check_favorites(self.pool(), user_id, project_id, entity_type, entity_ids)
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
        let set = favorite::check_span_favorites(self.pool(), user_id, project_id, span_ids)
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
        favorite::count_favorites(self.pool(), user_id, project_id)
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
        favorite::list_all_favorite_ids(self.pool(), user_id, project_id, entity_type, 10000)
            .await
            .map_err(Into::into)
    }

    async fn delete_favorites_by_entity(
        &self,
        entity_type: &str,
        entity_ids: &[String],
        project_id: &str,
    ) -> Result<u64, DataError> {
        favorite::delete_favorites_by_entity(self.pool(), project_id, entity_type, entity_ids)
            .await
            .map_err(Into::into)
    }

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
            self.pool(),
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
        file::get_file(self.pool(), project_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn file_exists(&self, project_id: &str, file_hash: &str) -> Result<bool, DataError> {
        file::file_exists(self.pool(), project_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn decrement_ref_count(
        &self,
        project_id: &str,
        file_hash: &str,
    ) -> Result<Option<i64>, DataError> {
        file::decrement_ref_count(self.pool(), project_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn delete_file(&self, project_id: &str, file_hash: &str) -> Result<bool, DataError> {
        file::delete_file(self.pool(), project_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn delete_project_files(&self, project_id: &str) -> Result<u64, DataError> {
        file::delete_project_files(self.pool(), project_id)
            .await
            .map_err(Into::into)
    }

    async fn insert_trace_file(
        &self,
        trace_id: &str,
        project_id: &str,
        file_hash: &str,
    ) -> Result<(), DataError> {
        file::insert_trace_file(self.pool(), trace_id, project_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn get_file_hashes_for_traces(
        &self,
        project_id: &str,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        file::get_file_hashes_for_traces(self.pool(), project_id, trace_ids)
            .await
            .map_err(Into::into)
    }

    async fn delete_trace_files(
        &self,
        project_id: &str,
        trace_ids: &[String],
    ) -> Result<u64, DataError> {
        file::delete_trace_files(self.pool(), project_id, trace_ids)
            .await
            .map_err(Into::into)
    }

    async fn get_project_storage_bytes(&self, project_id: &str) -> Result<i64, DataError> {
        file::get_project_storage_bytes(self.pool(), project_id)
            .await
            .map_err(Into::into)
    }

    async fn get_orphan_files(&self) -> Result<Vec<(String, String)>, DataError> {
        file::get_orphan_files(self.pool())
            .await
            .map_err(Into::into)
    }

    async fn get_org_file_storage_bytes(&self, org_id: &str) -> Result<i64, DataError> {
        file::get_org_file_storage_bytes(self.pool(), org_id)
            .await
            .map_err(Into::into)
    }

    async fn get_user_file_storage_bytes(&self, user_id: &str) -> Result<i64, DataError> {
        file::get_user_file_storage_bytes(self.pool(), user_id)
            .await
            .map_err(Into::into)
    }

    // ==================== API Key Operations ====================

    async fn create_api_key(
        &self,
        cache: Option<&CacheService>,
        org_id: &str,
        name: &str,
        key_hash: &str,
        key_prefix: &str,
        scope: ApiKeyScope,
        created_by: &str,
        expires_at: Option<i64>,
    ) -> Result<ApiKeyRow, DataError> {
        api_key::create_api_key(
            self.pool(),
            cache,
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
        cache: Option<&CacheService>,
        key_hash: &str,
    ) -> Result<Option<ApiKeyValidation>, DataError> {
        api_key::get_by_hash(self.pool(), cache, key_hash)
            .await
            .map_err(Into::into)
    }

    async fn list_api_keys(
        &self,
        cache: Option<&CacheService>,
        org_id: &str,
    ) -> Result<Vec<ApiKeyRow>, DataError> {
        api_key::list_for_org(self.pool(), cache, org_id)
            .await
            .map_err(Into::into)
    }

    async fn delete_api_key(
        &self,
        cache: Option<&CacheService>,
        id: &str,
        org_id: &str,
    ) -> Result<bool, DataError> {
        api_key::delete_api_key(self.pool(), cache, id, org_id)
            .await
            .map_err(Into::into)
    }

    async fn touch_api_key(&self, id: &str, threshold_secs: u64) -> Result<bool, DataError> {
        api_key::touch_api_key(self.pool(), id, threshold_secs)
            .await
            .map_err(Into::into)
    }

    async fn delete_api_keys_for_org(
        &self,
        cache: Option<&CacheService>,
        org_id: &str,
    ) -> Result<u64, DataError> {
        api_key::delete_for_org(self.pool(), cache, org_id)
            .await
            .map_err(Into::into)
    }

    async fn get_api_key_hashes_for_org(&self, org_id: &str) -> Result<Vec<String>, DataError> {
        api_key::get_hashes_for_org(self.pool(), org_id)
            .await
            .map_err(Into::into)
    }
}
