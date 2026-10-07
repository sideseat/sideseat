use super::*;

#[async_trait]
impl ApiKeyStore for PostgresRepository {
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
impl CredentialStore for PostgresRepository {
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
impl FavoriteStore for PostgresRepository {
    // ==================== Favorite Operations ====================

    async fn add_favorite(
        &self,
        user_id: &str,
        entity_type: &str,
        entity_id: &str,
        secondary_id: Option<&str>,
        project_id: &ProjectId,
    ) -> Result<bool, DataError> {
        tenant_transaction!(self, project_id, |connection| favorite::add_favorite(
            connection,
            user_id,
            project_id,
            entity_type,
            entity_id,
            secondary_id,
            self.0.clock().now().timestamp(),
        ))
    }

    async fn remove_favorite(
        &self,
        user_id: &str,
        entity_type: &str,
        entity_id: &str,
        secondary_id: Option<&str>,
        project_id: &ProjectId,
    ) -> Result<bool, DataError> {
        tenant_transaction!(self, project_id, |connection| favorite::remove_favorite(
            connection,
            user_id,
            project_id,
            entity_type,
            entity_id,
            secondary_id,
        ))
    }

    async fn check_favorites(
        &self,
        user_id: &str,
        entity_type: &str,
        entity_ids: &[String],
        project_id: &ProjectId,
    ) -> Result<Vec<String>, DataError> {
        let set = tenant_transaction!(self, project_id, |connection| {
            favorite::check_favorites(connection, user_id, project_id, entity_type, entity_ids)
        })?;
        Ok(set.into_iter().collect())
    }

    async fn check_span_favorites(
        &self,
        user_id: &str,
        span_ids: &[(String, String)],
        project_id: &ProjectId,
    ) -> Result<Vec<(String, String)>, DataError> {
        let set = tenant_transaction!(self, project_id, |connection| {
            favorite::check_span_favorites(connection, user_id, project_id, span_ids)
        })?;
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
        let count = tenant_transaction!(self, project_id, |connection| {
            favorite::count_favorites(connection, user_id, project_id)
        })?;
        Ok(count as i64)
    }

    async fn list_favorite_ids(
        &self,
        user_id: &str,
        entity_type: &str,
        project_id: &ProjectId,
    ) -> Result<Vec<String>, DataError> {
        // Use a reasonable default limit
        tenant_transaction!(self, project_id, |connection| {
            favorite::list_all_favorite_ids(connection, user_id, project_id, entity_type, 10000)
        })
    }

    async fn delete_favorites_by_entity(
        &self,
        entity_type: &str,
        entity_ids: &[String],
        project_id: &ProjectId,
    ) -> Result<u64, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            favorite::delete_favorites_by_entity(connection, project_id, entity_type, entity_ids)
        })
    }
}

#[async_trait]
impl DeletionJournal for PostgresRepository {
    async fn record_deleted_traces_journalled(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<(), DataError> {
        tenant_transaction!(self, project_id, |connection| {
            project::record_deleted_traces_journalled(
                connection,
                project_id,
                trace_ids,
                self.0.clock().now(),
            )
        })
    }

    async fn record_deleted_sessions_journalled(
        &self,
        project_id: &ProjectId,
        session_ids: &[String],
        trace_ids: &[String],
    ) -> Result<(), DataError> {
        tenant_transaction!(self, project_id, |connection| {
            project::record_deleted_sessions_journalled(
                connection,
                project_id,
                session_ids,
                trace_ids,
                self.0.clock().now(),
            )
        })
    }

    async fn record_pressure_eviction(
        &self,
        project_id: &ProjectId,
        spans: &[(String, String)],
    ) -> Result<Vec<(String, i64)>, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            project::record_pressure_eviction(connection, project_id, spans, self.0.clock().now())
        })
    }

    async fn record_deleted_spans_journalled(
        &self,
        project_id: &ProjectId,
        spans: &[(String, String)],
    ) -> Result<Vec<(String, i64)>, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            project::record_deleted_spans_journalled(
                connection,
                project_id,
                spans,
                self.0.clock().now(),
            )
        })
    }

    async fn claim_project_for_deletion_journalled(&self, id: &str) -> Result<bool, DataError> {
        let project_id = ProjectId::from(id);
        let claimed = tenant_transaction!(self, &project_id, |connection| {
            project::claim_project_for_deletion_journalled(connection, id, self.0.clock().now())
        })?;
        Ok(claimed)
    }

    async fn claim_organization_for_deletion_journalled(
        &self,
        id: &str,
    ) -> Result<bool, DataError> {
        let journal_tenant = ProjectId::from(id);
        tenant_transaction!(self, &journal_tenant, |connection| {
            project::claim_organization_for_deletion_journalled(
                connection,
                id,
                self.0.clock().now(),
            )
        })
    }

    async fn append_deletions(&self, records: &[DeletionRecord]) -> Result<(), DataError> {
        let Some(first) = records.first() else {
            return Ok(());
        };
        if records
            .iter()
            .any(|record| record.project_id != first.project_id)
        {
            return Err(DataError::Conflict(
                "a deletion-journal append must contain exactly one project".to_owned(),
            ));
        }
        tenant_transaction!(self, &first.project_id, |connection| {
            journal::append_deletions(connection, records)
        })
    }

    async fn deletions_since(
        &self,
        after_sequence: i64,
        limit: usize,
    ) -> Result<(Vec<(i64, DeletionRecord)>, i64), DataError> {
        maintenance_transaction!(self, |connection| {
            journal::deletions_since(connection, after_sequence, limit)
        })
    }

    async fn deletion_is_journaled(
        &self,
        project_id: &ProjectId,
        scope: DeletionScope,
        target_id: &str,
        span_id: Option<&str>,
    ) -> Result<bool, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            journal::deletion_is_journaled(connection, project_id, scope, target_id, span_id)
        })
    }

    async fn journaled_spans_among(
        &self,
        project_id: &ProjectId,
        spans: &[(String, String)],
    ) -> Result<std::collections::HashSet<(String, String)>, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            journal::journaled_spans_among(connection, project_id, spans)
        })
    }

    async fn journaled_span_deletions_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<(String, String)>, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            journal::journaled_span_deletions_for_traces(connection, project_id, trace_ids)
        })
    }
}

#[async_trait]
impl StagedPayloadStore for PostgresRepository {
    async fn create_staged_payload(&self, payload: &StagedPayload) -> Result<i64, DataError> {
        tenant_transaction!(self, &payload.project_id, |connection| {
            staging::create(connection, payload)
        })
    }

    async fn staged_sequence_state(
        &self,
        seq: i64,
    ) -> Result<sideseat_ports::types::StagedSequenceState, DataError> {
        maintenance_transaction!(self, |connection| staging::sequence_state(connection, seq))
    }

    async fn record_staging_anomaly(
        &self,
        id: &str,
        seq: i64,
        detected_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), DataError> {
        maintenance_transaction!(self, |connection| {
            staging::record_anomaly(connection, id, seq, detected_at)
        })
    }

    async fn get_staged_payload(&self, id: &str) -> Result<Option<StagedPayload>, DataError> {
        maintenance_transaction!(self, |connection| staging::get(connection, id))
    }

    async fn pending_staged_payloads(&self, limit: usize) -> Result<Vec<StagedPayload>, DataError> {
        maintenance_transaction!(self, |connection| staging::pending(connection, limit))
    }

    async fn increment_staged_redrive_attempts(&self, id: &str) -> Result<u32, DataError> {
        maintenance_transaction!(self, |connection| {
            staging::increment_attempts(connection, id)
        })
    }

    async fn mark_staged_unconfirmed(&self, id: &str) -> Result<(), DataError> {
        maintenance_transaction!(self, |connection| {
            staging::mark_unconfirmed(connection, id)
        })
    }

    async fn delete_staged_payload(&self, id: &str) -> Result<(), DataError> {
        maintenance_transaction!(self, |connection| staging::delete(connection, id))
    }

    async fn delete_project_staged_payloads(
        &self,
        project_id: &ProjectId,
    ) -> Result<u64, DataError> {
        maintenance_transaction!(self, |connection| {
            staging::delete_project(connection, project_id)
        })
    }
}

#[async_trait]
impl StorageGovernance for PostgresRepository {
    async fn active_project_hold(
        &self,
        project_id: &ProjectId,
        now: DateTime<Utc>,
    ) -> Result<Option<ProjectHold>, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            governance::active_hold(connection, project_id, now)
        })
    }

    async fn list_active_project_holds(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<ProjectHold>, DataError> {
        maintenance_transaction!(self, |connection| {
            governance::list_active_holds(connection, now, limit)
        })
    }

    async fn set_project_hold(
        &self,
        project_id: &ProjectId,
        hold_until: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<ProjectHold, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            governance::set_hold(connection, project_id, hold_until, now)
        })
    }

    async fn clear_project_hold(&self, project_id: &ProjectId) -> Result<bool, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            governance::clear_hold(connection, project_id)
        })
    }

    async fn acquire_project_maintenance(
        &self,
        project_id: &ProjectId,
        owner: &str,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
    ) -> Result<bool, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            governance::acquire_maintenance(connection, project_id, owner, now, lease_until)
        })
    }

    async fn release_project_maintenance(
        &self,
        project_id: &ProjectId,
        owner: &str,
    ) -> Result<(), DataError> {
        tenant_transaction!(self, project_id, |connection| {
            governance::release_maintenance(connection, project_id, owner)
        })
    }

    async fn reserve_project_storage(
        &self,
        project_id: &ProjectId,
        additional_bytes: u64,
        ordinary_limit_bytes: u64,
        now: DateTime<Utc>,
    ) -> Result<Option<ProjectStorageUsage>, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            governance::reserve_storage(
                connection,
                project_id,
                additional_bytes,
                ordinary_limit_bytes,
                now,
            )
        })
    }

    async fn project_storage_usage(
        &self,
        project_id: &ProjectId,
    ) -> Result<ProjectStorageUsage, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            governance::usage(connection, project_id)
        })
    }

    async fn replace_project_storage_usage(
        &self,
        project_id: &ProjectId,
        logical_bytes: u64,
        now: DateTime<Utc>,
    ) -> Result<ProjectStorageUsage, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            governance::replace_usage(connection, project_id, logical_bytes, now)
        })
    }

    async fn held_transactional_bytes(&self, project_id: &ProjectId) -> Result<u64, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            governance::held_transactional_bytes(connection, project_id)
        })
    }

    async fn storage_project_ids(&self, limit: usize) -> Result<Vec<ProjectId>, DataError> {
        maintenance_transaction!(self, |connection| {
            governance::project_ids(connection, limit)
        })
    }
}
