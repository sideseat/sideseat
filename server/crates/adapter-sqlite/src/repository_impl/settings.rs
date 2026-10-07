use super::*;

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
            self.0.clock().now().timestamp(),
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

    async fn touch_api_key(
        &self,
        id: &str,
        key_hash: &str,
        threshold_secs: u64,
    ) -> Result<bool, DataError> {
        api_key::touch_api_key(
            self.0.pool(),
            self.0.cache(),
            id,
            key_hash,
            threshold_secs,
            self.0.clock().now().timestamp(),
        )
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
            self.0.clock().now().timestamp(),
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
            self.0.clock().now().timestamp(),
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
        project_id: Option<&ProjectId>,
        access: &str,
        created_by: Option<&str>,
    ) -> Result<CredentialPermissionRow, DataError> {
        credential_permissions::create_credential_permission(
            self.0.pool(),
            id,
            credential_id,
            org_id,
            project_id.map(ProjectId::as_str),
            access,
            created_by,
            self.0.clock().now().timestamp(),
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
        project_id: &ProjectId,
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
        project_id: &ProjectId,
    ) -> Result<bool, DataError> {
        favorite::add_favorite(
            self.0.pool(),
            user_id,
            project_id,
            entity_type,
            entity_id,
            secondary_id,
            self.0.clock().now().timestamp(),
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
        project_id: &ProjectId,
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
        project_id: &ProjectId,
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
        project_id: &ProjectId,
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

    async fn count_favorites(
        &self,
        user_id: &str,
        project_id: &ProjectId,
    ) -> Result<i64, DataError> {
        favorite::count_favorites(self.0.pool(), user_id, project_id)
            .await
            .map(|c| c as i64)
            .map_err(Into::into)
    }

    async fn list_favorite_ids(
        &self,
        user_id: &str,
        entity_type: &str,
        project_id: &ProjectId,
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
        project_id: &ProjectId,
    ) -> Result<u64, DataError> {
        favorite::delete_favorites_by_entity(self.0.pool(), project_id, entity_type, entity_ids)
            .await
            .map_err(Into::into)
    }
}

#[async_trait]
impl DeletionJournal for SqliteRepository {
    async fn record_deleted_traces_journalled(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<(), DataError> {
        project::record_deleted_traces_journalled(
            self.0.pool(),
            project_id,
            trace_ids,
            self.0.clock().now(),
        )
        .await
        .map_err(Into::into)
    }

    async fn record_deleted_sessions_journalled(
        &self,
        project_id: &ProjectId,
        session_ids: &[String],
        trace_ids: &[String],
    ) -> Result<(), DataError> {
        project::record_deleted_sessions_journalled(
            self.0.pool(),
            project_id,
            session_ids,
            trace_ids,
            self.0.clock().now(),
        )
        .await
        .map_err(Into::into)
    }

    async fn record_pressure_eviction(
        &self,
        project_id: &ProjectId,
        spans: &[(String, String)],
    ) -> Result<Vec<(String, i64)>, DataError> {
        project::record_pressure_eviction(self.0.pool(), project_id, spans, self.0.clock().now())
            .await
            .map_err(Into::into)
    }

    async fn record_deleted_spans_journalled(
        &self,
        project_id: &ProjectId,
        spans: &[(String, String)],
    ) -> Result<Vec<(String, i64)>, DataError> {
        project::record_deleted_spans_journalled(
            self.0.pool(),
            project_id,
            spans,
            self.0.clock().now(),
        )
        .await
        .map_err(Into::into)
    }

    async fn claim_project_for_deletion_journalled(&self, id: &str) -> Result<bool, DataError> {
        project::claim_project_for_deletion_journalled(self.0.pool(), id, self.0.clock().now())
            .await
            .map_err(Into::into)
    }

    async fn claim_organization_for_deletion_journalled(
        &self,
        id: &str,
    ) -> Result<bool, DataError> {
        project::claim_organization_for_deletion_journalled(self.0.pool(), id, self.0.clock().now())
            .await
            .map_err(Into::into)
    }

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
        project_id: &ProjectId,
        scope: DeletionScope,
        target_id: &str,
        span_id: Option<&str>,
    ) -> Result<bool, DataError> {
        journal::deletion_is_journaled(self.0.pool(), project_id, scope, target_id, span_id)
            .await
            .map_err(Into::into)
    }

    async fn journaled_spans_among(
        &self,
        project_id: &ProjectId,
        spans: &[(String, String)],
    ) -> Result<std::collections::HashSet<(String, String)>, DataError> {
        journal::journaled_spans_among(self.0.pool(), project_id, spans)
            .await
            .map_err(Into::into)
    }

    async fn journaled_span_deletions_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<(String, String)>, DataError> {
        journal::journaled_span_deletions_for_traces(self.0.pool(), project_id, trace_ids)
            .await
            .map_err(Into::into)
    }
}

#[async_trait]
impl StagedPayloadStore for SqliteRepository {
    async fn create_staged_payload(&self, payload: &StagedPayload) -> Result<i64, DataError> {
        staging::create(self.0.pool(), payload)
            .await
            .map_err(Into::into)
    }

    async fn staged_sequence_state(
        &self,
        seq: i64,
    ) -> Result<sideseat_ports::types::StagedSequenceState, DataError> {
        staging::sequence_state(self.0.pool(), seq)
            .await
            .map_err(Into::into)
    }

    async fn record_staging_anomaly(
        &self,
        id: &str,
        seq: i64,
        detected_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), DataError> {
        staging::record_anomaly(self.0.pool(), id, seq, detected_at)
            .await
            .map_err(Into::into)
    }

    async fn get_staged_payload(&self, id: &str) -> Result<Option<StagedPayload>, DataError> {
        staging::get(self.0.pool(), id).await.map_err(Into::into)
    }

    async fn pending_staged_payloads(&self, limit: usize) -> Result<Vec<StagedPayload>, DataError> {
        staging::pending(self.0.pool(), limit)
            .await
            .map_err(Into::into)
    }

    async fn increment_staged_redrive_attempts(&self, id: &str) -> Result<u32, DataError> {
        staging::increment_attempts(self.0.pool(), id)
            .await
            .map_err(Into::into)
    }

    async fn mark_staged_unconfirmed(&self, id: &str) -> Result<(), DataError> {
        staging::mark_unconfirmed(self.0.pool(), id)
            .await
            .map_err(Into::into)
    }

    async fn delete_staged_payload(&self, id: &str) -> Result<(), DataError> {
        staging::delete(self.0.pool(), id).await.map_err(Into::into)
    }

    async fn delete_project_staged_payloads(
        &self,
        project_id: &ProjectId,
    ) -> Result<u64, DataError> {
        staging::delete_project(self.0.pool(), project_id)
            .await
            .map_err(Into::into)
    }
}

#[async_trait]
impl StorageGovernance for SqliteRepository {
    async fn active_project_hold(
        &self,
        project_id: &ProjectId,
        now: DateTime<Utc>,
    ) -> Result<Option<ProjectHold>, DataError> {
        governance::active_hold(self.0.pool(), project_id, now)
            .await
            .map_err(Into::into)
    }

    async fn list_active_project_holds(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<ProjectHold>, DataError> {
        governance::list_active_holds(self.0.pool(), now, limit)
            .await
            .map_err(Into::into)
    }

    async fn set_project_hold(
        &self,
        project_id: &ProjectId,
        hold_until: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<ProjectHold, DataError> {
        governance::set_hold(self.0.pool(), project_id, hold_until, now)
            .await
            .map_err(Into::into)
    }

    async fn clear_project_hold(&self, project_id: &ProjectId) -> Result<bool, DataError> {
        governance::clear_hold(self.0.pool(), project_id)
            .await
            .map_err(Into::into)
    }

    async fn acquire_project_maintenance(
        &self,
        project_id: &ProjectId,
        owner: &str,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
    ) -> Result<bool, DataError> {
        governance::acquire_maintenance(self.0.pool(), project_id, owner, now, lease_until)
            .await
            .map_err(Into::into)
    }

    async fn release_project_maintenance(
        &self,
        project_id: &ProjectId,
        owner: &str,
    ) -> Result<(), DataError> {
        governance::release_maintenance(self.0.pool(), project_id, owner)
            .await
            .map_err(Into::into)
    }

    async fn reserve_project_storage(
        &self,
        project_id: &ProjectId,
        additional_bytes: u64,
        ordinary_limit_bytes: u64,
        now: DateTime<Utc>,
    ) -> Result<Option<ProjectStorageUsage>, DataError> {
        governance::reserve_storage(
            self.0.pool(),
            project_id,
            additional_bytes,
            ordinary_limit_bytes,
            now,
        )
        .await
        .map_err(Into::into)
    }

    async fn project_storage_usage(
        &self,
        project_id: &ProjectId,
    ) -> Result<ProjectStorageUsage, DataError> {
        governance::usage(self.0.pool(), project_id)
            .await
            .map_err(Into::into)
    }

    async fn replace_project_storage_usage(
        &self,
        project_id: &ProjectId,
        logical_bytes: u64,
        now: DateTime<Utc>,
    ) -> Result<ProjectStorageUsage, DataError> {
        governance::replace_usage(self.0.pool(), project_id, logical_bytes, now)
            .await
            .map_err(Into::into)
    }

    async fn held_transactional_bytes(&self, project_id: &ProjectId) -> Result<u64, DataError> {
        governance::held_transactional_bytes(self.0.pool(), project_id)
            .await
            .map_err(Into::into)
    }

    async fn storage_project_ids(&self, limit: usize) -> Result<Vec<ProjectId>, DataError> {
        governance::project_ids(self.0.pool(), limit)
            .await
            .map_err(Into::into)
    }
}
