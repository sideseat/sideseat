use super::*;

#[async_trait]
impl FileMetaStore for PostgresRepository {
    // ==================== File Operations ====================

    async fn restore_association_trace_ids(
        &self,
        project_id: &ProjectId,
        after_trace_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            restore::association_trace_ids(connection, project_id, after_trace_id, limit)
        })
    }

    async fn upsert_file(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
        media_type: Option<&str>,
        size_bytes: i64,
        hash_algo: &str,
    ) -> Result<i64, DataError> {
        tenant_transaction!(self, project_id, |connection| file::upsert_file(
            connection,
            project_id,
            file_hash,
            media_type,
            size_bytes,
            hash_algo,
            self.0.clock().now().timestamp(),
        ))
    }

    async fn get_file(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<Option<FileRow>, DataError> {
        tenant_transaction!(self, project_id, |connection| file::get_file(
            connection, project_id, file_hash
        ))
    }

    async fn file_exists(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<bool, DataError> {
        tenant_transaction!(self, project_id, |connection| file::file_exists(
            connection, project_id, file_hash
        ))
    }

    async fn decrement_ref_count(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<Option<i64>, DataError> {
        tenant_transaction!(self, project_id, |connection| file::decrement_ref_count(
            connection,
            project_id,
            file_hash,
            self.0.clock().now().timestamp(),
        ))
    }

    async fn delete_file(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<bool, DataError> {
        tenant_transaction!(self, project_id, |connection| file::delete_file(
            connection, project_id, file_hash
        ))
    }

    async fn delete_project_files(&self, project_id: &ProjectId) -> Result<u64, DataError> {
        tenant_transaction!(self, project_id, |connection| file::delete_project_files(
            connection, project_id
        ))
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
        tenant_transaction!(self, project_id, |connection| file::associate_file(
            connection,
            trace_id,
            project_id,
            file_hash,
            media_type,
            size_bytes,
            hash_algo,
            self.0.clock().now().timestamp(),
        ))
    }

    async fn get_file_reference_counts_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<(String, i64)>, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            file::get_file_reference_counts_for_traces(connection, project_id, trace_ids)
        })
    }

    async fn associate_existing_file(
        &self,
        trace_id: &str,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<bool, DataError> {
        tenant_transaction!(
            self,
            project_id,
            |connection| file::associate_existing_file(
                connection,
                trace_id,
                project_id,
                file_hash,
                self.0.clock().now().timestamp(),
            )
        )
    }

    async fn get_stale_claimed_files(
        &self,
        older_than_secs: i64,
    ) -> Result<Vec<(String, String, i64)>, DataError> {
        maintenance_transaction!(self, |connection| file::get_stale_claimed_files(
            connection,
            older_than_secs,
            self.0.clock().now().timestamp(),
        ))
    }

    async fn reclaim_stale_file(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
        observed_deleting_at: i64,
    ) -> Result<bool, DataError> {
        tenant_transaction!(self, project_id, |connection| file::reclaim_stale_file(
            connection,
            project_id,
            file_hash,
            observed_deleting_at,
            self.0.clock().now().timestamp(),
        ))
    }

    async fn claim_file_for_deletion(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<bool, DataError> {
        tenant_transaction!(
            self,
            project_id,
            |connection| file::claim_file_for_deletion(
                connection,
                project_id,
                file_hash,
                self.0.clock().now().timestamp(),
            )
        )
    }

    async fn release_deletion_claim(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<(), DataError> {
        tenant_transaction!(self, project_id, |connection| {
            file::release_deletion_claim(connection, project_id, file_hash)
        })
    }

    async fn restore_orphan_metadata(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
        media_type: Option<&str>,
        size_bytes: i64,
        hash_algo: &str,
    ) -> Result<(), DataError> {
        tenant_transaction!(
            self,
            project_id,
            |connection| file::restore_orphan_metadata(
                connection,
                project_id,
                file_hash,
                media_type,
                size_bytes,
                hash_algo,
                self.0.clock().now().timestamp(),
            )
        )
    }

    async fn delete_file_if_unreferenced(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<bool, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            file::delete_file_if_unreferenced(connection, project_id, file_hash)
        })
    }

    async fn sync_ref_count(
        &self,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<Option<i64>, DataError> {
        tenant_transaction!(self, project_id, |connection| file::sync_ref_count(
            connection,
            project_id,
            file_hash,
            self.0.clock().now().timestamp(),
        ))
    }

    async fn insert_trace_file(
        &self,
        trace_id: &str,
        project_id: &ProjectId,
        file_hash: &str,
    ) -> Result<(), DataError> {
        tenant_transaction!(self, project_id, |connection| file::insert_trace_file(
            connection, trace_id, project_id, file_hash
        ))
    }

    async fn get_file_hashes_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            file::get_file_hashes_for_traces(connection, project_id, trace_ids)
        })
    }

    async fn confirm_trace_file_associations(
        &self,
        associations: &[(String, String, String)],
    ) -> Result<u64, DataError> {
        let mut by_project = BTreeMap::<&str, Vec<(String, String, String)>>::new();
        for association in associations {
            by_project
                .entry(association.0.as_str())
                .or_default()
                .push(association.clone());
        }

        let mut confirmed = 0;
        for (project_id, project_associations) in by_project {
            let project_id = ProjectId::from(project_id);
            confirmed += tenant_transaction!(self, &project_id, |connection| {
                file::confirm_trace_file_associations(connection, &project_associations)
            })?;
        }
        Ok(confirmed)
    }

    async fn release_trace_file_association(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        file_hash: &str,
    ) -> Result<bool, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            file::release_trace_file_association(connection, project_id, trace_id, file_hash)
        })
    }

    async fn delete_trace_files(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        tenant_transaction!(self, project_id, |connection| file::delete_trace_files(
            connection, project_id, trace_ids
        ))
    }

    async fn record_retention_cleanup(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<(String, i64)>, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            file::record_retention_cleanup(
                connection,
                project_id,
                trace_ids,
                self.0.clock().now().timestamp(),
            )
        })
    }

    async fn claim_retention_cleanup(
        &self,
        limit: i64,
        lease_secs: i64,
    ) -> Result<Vec<(String, String, i64)>, DataError> {
        maintenance_transaction!(self, |connection| file::claim_retention_cleanup(
            connection,
            limit,
            lease_secs,
            self.0.clock().now().timestamp(),
        ))
    }

    async fn complete_retention_cleanup(
        &self,
        project_id: &ProjectId,
        completed: &[(String, i64)],
    ) -> Result<(), DataError> {
        tenant_transaction!(self, project_id, |connection| {
            file::complete_retention_cleanup(connection, project_id, completed)
        })
    }

    async fn restore_durable_trace_file(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        file_hash: &str,
    ) -> Result<(), DataError> {
        tenant_transaction!(self, project_id, |connection| {
            file::restore_durable_trace_file(connection, project_id, trace_id, file_hash)
        })
    }

    async fn release_trace_files_except(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        keep: &[String],
    ) -> Result<Vec<String>, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            file::release_trace_files_except(connection, project_id, trace_id, keep)
        })
    }

    async fn get_project_storage_bytes(&self, project_id: &ProjectId) -> Result<i64, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            file::get_project_storage_bytes(connection, project_id)
        })
    }

    async fn get_orphan_files(&self) -> Result<Vec<(String, String)>, DataError> {
        maintenance_transaction!(self, |connection| file::get_orphan_files(connection))
    }

    async fn get_org_file_storage_bytes(&self, org_id: &str) -> Result<i64, DataError> {
        maintenance_transaction!(self, |connection| {
            file::get_org_file_storage_bytes(connection, org_id)
        })
    }

    async fn get_user_file_storage_bytes(&self, user_id: &str) -> Result<i64, DataError> {
        maintenance_transaction!(self, |connection| {
            file::get_user_file_storage_bytes(connection, user_id)
        })
    }
}

#[async_trait]
impl ContentBodyStore for PostgresRepository {
    async fn register_content_bodies(
        &self,
        objects: &[ContentBodyObject],
    ) -> Result<Vec<ContentBodyObject>, DataError> {
        let mut by_project = BTreeMap::<&ProjectId, Vec<ContentBodyObject>>::new();
        for object in objects {
            by_project
                .entry(&object.project_id)
                .or_default()
                .push(object.clone());
        }

        let mut inserted = Vec::new();
        for (project_id, objects) in by_project {
            inserted.extend(tenant_transaction!(self, project_id, |connection| {
                body::register(connection, &objects, self.0.clock().now())
            })?);
        }
        Ok(inserted)
    }

    async fn unresolved_span_bodies(
        &self,
        associations: &[SpanBodyAssociation],
    ) -> Result<Vec<SpanBodyAssociation>, DataError> {
        let mut by_project = BTreeMap::<&ProjectId, Vec<SpanBodyAssociation>>::new();
        for association in associations {
            by_project
                .entry(&association.project_id)
                .or_default()
                .push(association.clone());
        }

        let mut unresolved = Vec::new();
        for (project_id, associations) in by_project {
            unresolved.extend(tenant_transaction!(self, project_id, |connection| {
                body::unresolved(connection, &associations)
            })?);
        }
        Ok(unresolved)
    }

    async fn stage_span_bodies(
        &self,
        associations: &[SpanBodyAssociation],
    ) -> Result<u64, DataError> {
        let mut by_project = BTreeMap::<&ProjectId, Vec<SpanBodyAssociation>>::new();
        for association in associations {
            by_project
                .entry(&association.project_id)
                .or_default()
                .push(association.clone());
        }

        let mut staged = 0;
        for (project_id, associations) in by_project {
            staged += tenant_transaction!(self, project_id, |connection| {
                body::stage(connection, &associations)
            })?;
        }
        Ok(staged)
    }

    async fn confirm_span_bodies(
        &self,
        associations: &[SpanBodyAssociation],
    ) -> Result<u64, DataError> {
        let mut by_project = BTreeMap::<&ProjectId, Vec<SpanBodyAssociation>>::new();
        for association in associations {
            by_project
                .entry(&association.project_id)
                .or_default()
                .push(association.clone());
        }

        let mut confirmed = 0;
        for (project_id, associations) in by_project {
            confirmed += tenant_transaction!(self, project_id, |connection| {
                body::confirm(connection, &associations)
            })?;
        }
        Ok(confirmed)
    }

    async fn release_span_body(
        &self,
        association: &SpanBodyAssociation,
    ) -> Result<bool, DataError> {
        tenant_transaction!(self, &association.project_id, |connection| {
            body::release(connection, association)
        })
    }

    async fn get_span_body_hash(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        span_id: &str,
        field: SpanBodyField,
    ) -> Result<Option<String>, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            body::get_hash(connection, project_id, trace_id, span_id, field)
        })
    }

    async fn get_orphan_content_bodies(
        &self,
        older_than: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<(ProjectId, String)>, DataError> {
        maintenance_transaction!(self, |connection| {
            body::orphans(connection, older_than, limit)
        })
    }

    async fn get_stale_claimed_content_bodies(
        &self,
        older_than: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<(ProjectId, String)>, DataError> {
        maintenance_transaction!(self, |connection| {
            body::stale_claims(connection, older_than, limit)
        })
    }

    async fn claim_content_body_for_deletion(
        &self,
        project_id: &ProjectId,
        body_hash: &str,
    ) -> Result<bool, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            body::claim_for_deletion(connection, project_id, body_hash, self.0.clock().now())
        })
    }

    async fn release_content_body_deletion_claim(
        &self,
        project_id: &ProjectId,
        body_hash: &str,
    ) -> Result<(), DataError> {
        tenant_transaction!(self, project_id, |connection| {
            body::release_deletion_claim(connection, project_id, body_hash)
        })
    }

    async fn delete_claimed_content_body(
        &self,
        project_id: &ProjectId,
        body_hash: &str,
    ) -> Result<bool, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            body::delete_claimed(connection, project_id, body_hash)
        })
    }

    async fn delete_span_bodies(
        &self,
        project_id: &ProjectId,
        spans: &[(String, String)],
    ) -> Result<Vec<String>, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            body::delete_spans(connection, project_id, spans)
        })
    }

    async fn delete_trace_bodies(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            body::delete_traces(connection, project_id, trace_ids)
        })
    }

    async fn delete_project_bodies(
        &self,
        project_id: &ProjectId,
    ) -> Result<Vec<String>, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            body::delete_project(connection, project_id)
        })
    }

    async fn reconcile_span_bodies(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
        keep: &[SpanBodyAssociation],
    ) -> Result<Vec<String>, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            body::reconcile(connection, project_id, trace_ids, keep)
        })
    }

    async fn content_body_backfill_progress(
        &self,
        project_id: &ProjectId,
    ) -> Result<Option<ContentBodyBackfillProgress>, DataError> {
        tenant_transaction!(self, project_id, |connection| {
            body::backfill_progress(connection, project_id)
        })
    }

    async fn save_content_body_backfill_progress(
        &self,
        progress: &ContentBodyBackfillProgress,
    ) -> Result<(), DataError> {
        tenant_transaction!(self, &progress.project_id, |connection| {
            body::save_backfill_progress(connection, progress)
        })
    }

    async fn reset_content_body_backfill(&self, project_id: &ProjectId) -> Result<(), DataError> {
        tenant_transaction!(self, project_id, |connection| {
            body::reset_backfill(connection, project_id, self.0.clock().now())
        })
    }
}
