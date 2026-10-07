use super::*;

#[async_trait]
impl FileMetaStore for SqliteRepository {
    // ==================== File Operations ====================

    async fn restore_association_trace_ids(
        &self,
        project_id: &ProjectId,
        after_trace_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>, DataError> {
        restore::association_trace_ids(self.0.pool(), project_id, after_trace_id, limit)
            .await
            .map_err(Into::into)
    }

    async fn upsert_file(
        &self,
        project_id: &ProjectId,
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
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn get_file(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<Option<FileRow>, DataError> {
        file::get_file(self.0.pool(), project_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn file_exists(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<bool, DataError> {
        file::file_exists(self.0.pool(), project_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn decrement_ref_count(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<Option<i64>, DataError> {
        file::decrement_ref_count(
            self.0.pool(),
            project_id,
            file_hash,
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn delete_file(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<bool, DataError> {
        file::delete_file(self.0.pool(), project_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn delete_project_files(&self, project_id: &ProjectId) -> Result<u64, DataError> {
        file::delete_project_files(self.0.pool(), project_id)
            .await
            .map_err(Into::into)
    }

    async fn associate_file(
        &self,
        trace_id: &str,
        project_id: &ProjectId,
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
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn get_file_reference_counts_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<(String, i64)>, DataError> {
        file::get_file_reference_counts_for_traces(self.0.pool(), project_id, trace_ids)
            .await
            .map_err(Into::into)
    }

    async fn associate_existing_file(
        &self,
        trace_id: &str,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<bool, DataError> {
        file::associate_existing_file(
            self.0.pool(),
            trace_id,
            project_id,
            file_hash,
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn get_stale_claimed_files(
        &self,
        older_than_secs: i64,
    ) -> Result<Vec<(String, String, i64)>, DataError> {
        file::get_stale_claimed_files(
            self.0.pool(),
            older_than_secs,
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn reclaim_stale_file(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
        observed_deleting_at: i64,
    ) -> Result<bool, DataError> {
        file::reclaim_stale_file(
            self.0.pool(),
            project_id,
            file_hash,
            observed_deleting_at,
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn claim_file_for_deletion(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<bool, DataError> {
        file::claim_file_for_deletion(
            self.0.pool(),
            project_id,
            file_hash,
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn release_deletion_claim(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<(), DataError> {
        file::release_deletion_claim(self.0.pool(), project_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn restore_orphan_metadata(
        &self,
        project_id: &ProjectId,
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
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn delete_file_if_unreferenced(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<bool, DataError> {
        file::delete_file_if_unreferenced(self.0.pool(), project_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn sync_ref_count(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<Option<i64>, DataError> {
        file::sync_ref_count(
            self.0.pool(),
            project_id,
            file_hash,
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn insert_trace_file(
        &self,
        trace_id: &str,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<(), DataError> {
        file::insert_trace_file(self.0.pool(), trace_id, project_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn get_file_hashes_for_traces(
        &self,
        project_id: &ProjectId,
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
        project_id: &ProjectId,
        trace_id: &str,
        file_hash: &str,
    ) -> Result<bool, DataError> {
        file::release_trace_file_association(self.0.pool(), project_id, trace_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn delete_trace_files(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        file::delete_trace_files(self.0.pool(), project_id, trace_ids)
            .await
            .map_err(Into::into)
    }

    async fn record_retention_cleanup(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<(String, i64)>, DataError> {
        file::record_retention_cleanup(
            self.0.pool(),
            project_id,
            trace_ids,
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn claim_retention_cleanup(
        &self,
        limit: i64,
        lease_secs: i64,
    ) -> Result<Vec<(String, String, i64)>, DataError> {
        file::claim_retention_cleanup(
            self.0.pool(),
            limit,
            lease_secs,
            self.0.clock().now().timestamp(),
        )
        .await
        .map_err(Into::into)
    }

    async fn complete_retention_cleanup(
        &self,
        project_id: &ProjectId,
        completed: &[(String, i64)],
    ) -> Result<(), DataError> {
        file::complete_retention_cleanup(self.0.pool(), project_id, completed)
            .await
            .map_err(Into::into)
    }

    async fn restore_durable_trace_file(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        file_hash: &str,
    ) -> Result<(), DataError> {
        file::restore_durable_trace_file(self.0.pool(), project_id, trace_id, file_hash)
            .await
            .map_err(Into::into)
    }

    async fn release_trace_files_except(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        keep: &[String],
    ) -> Result<Vec<String>, DataError> {
        file::release_trace_files_except(self.0.pool(), project_id, trace_id, keep)
            .await
            .map_err(Into::into)
    }

    async fn get_project_storage_bytes(&self, project_id: &ProjectId) -> Result<i64, DataError> {
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
impl ContentBodyStore for SqliteRepository {
    async fn retire_span_body_associations(&self, limit: usize) -> Result<u64, DataError> {
        body::retire_associations(self.0.pool(), limit)
            .await
            .map_err(Into::into)
    }

    async fn get_orphan_content_bodies(
        &self,
        older_than: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<(ProjectId, String)>, DataError> {
        body::orphans(self.0.pool(), older_than, limit)
            .await
            .map_err(Into::into)
    }

    async fn get_stale_claimed_content_bodies(
        &self,
        older_than: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<(ProjectId, String)>, DataError> {
        body::stale_claims(self.0.pool(), older_than, limit)
            .await
            .map_err(Into::into)
    }

    async fn claim_content_body_for_deletion(
        &self,
        project_id: &ProjectId,
        body_hash: &str,
    ) -> Result<bool, DataError> {
        body::claim_for_deletion(self.0.pool(), project_id, body_hash, self.0.clock().now())
            .await
            .map_err(Into::into)
    }

    async fn release_content_body_deletion_claim(
        &self,
        project_id: &ProjectId,
        body_hash: &str,
    ) -> Result<(), DataError> {
        body::release_deletion_claim(self.0.pool(), project_id, body_hash)
            .await
            .map_err(Into::into)
    }

    async fn delete_claimed_content_body(
        &self,
        project_id: &ProjectId,
        body_hash: &str,
    ) -> Result<bool, DataError> {
        body::delete_claimed(self.0.pool(), project_id, body_hash)
            .await
            .map_err(Into::into)
    }

    async fn delete_project_bodies(
        &self,
        project_id: &ProjectId,
    ) -> Result<Vec<String>, DataError> {
        body::delete_project(self.0.pool(), project_id)
            .await
            .map_err(Into::into)
    }
}
