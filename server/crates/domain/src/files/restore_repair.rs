//! Rebuilding file ownership after a restore, from the references surviving analytics rows hold.

use super::*;

impl FileService {
    /// Rebuild uploaded-file ownership from the references in surviving analytics rows.
    ///
    /// This is the restore-specific direction of reconciliation: ordinary retention starts from
    /// transactional associations and releases those no survivor needs, while a mismatched restore can
    /// have the survivor and the bytes but have lost the association itself. The association is restored
    /// as durable and the cached count is derived from the resulting rows before any orphan GC may run.
    ///
    /// A surviving row whose bytes are absent is not fabricated. Its hash is returned in
    /// [`FileRestoreRepairReport::missing_content`] so an operator and the API can distinguish corruption
    /// from a reference that never existed.
    pub async fn repair_trace_associations_after_restore(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
        analytics: &dyn sideseat_ports::traits::SurvivorReferences,
    ) -> Result<FileRestoreRepairReport, FileServiceError> {
        let mut report = FileRestoreRepairReport::default();
        if !self.config.enabled {
            return Ok(report);
        }

        for trace_id in trace_ids {
            report.traces_scanned += 1;
            let fields = analytics
                .file_reference_fields_for_traces(project_id, std::slice::from_ref(trace_id))
                .await?;
            let mut uris = Vec::new();
            for field in &fields {
                collect_file_references_in_str(field, &mut uris);
            }

            let mut references = std::collections::BTreeMap::<String, Option<String>>::new();
            for uri in uris {
                if let Some(parsed) = parse_file_uri(&uri) {
                    references
                        .entry(parsed.hash.to_owned())
                        .or_insert_with(|| parsed.media_type.map(str::to_owned));
                }
            }
            for hash in raw_media_of_survivors(project_id, trace_id, analytics).await? {
                references.entry(hash).or_insert(None);
            }
            report.references_scanned += references.len() as u64;

            let associated = self
                .database
                .get_file_hashes_for_traces(project_id, std::slice::from_ref(trace_id))
                .await?
                .into_iter()
                .collect::<std::collections::HashSet<_>>();

            for (hash, media_type) in references {
                // The stores were restored from backups taken apart, so the row and the bytes may name
                // different incarnations, or the bytes have no row: the bytes found are made the row's.
                let row = self.database.get_file(project_id, &hash).await?;
                let stored = self.storage.incarnations(project_id, &hash).await?;
                let current = row.as_ref().filter(|row| stored.contains(&row.id));
                let incarnation = if let Some(row) = current {
                    row.id
                } else {
                    let Some(&found) = stored.last() else {
                        report.missing_content.push(MissingFileReference {
                            project_id: project_id.clone(),
                            trace_id: trace_id.clone(),
                            hash,
                        });
                        continue;
                    };
                    let bytes = match self
                        .storage
                        .get(project_id, ObjectName::new(&hash, found))
                        .await
                    {
                        Ok(bytes) => bytes,
                        Err(FileStorageError::NotFound { .. }) => {
                            report.missing_content.push(MissingFileReference {
                                project_id: project_id.clone(),
                                trace_id: trace_id.clone(),
                                hash,
                            });
                            continue;
                        }
                        Err(error) => return Err(error.into()),
                    };
                    let incarnation = match &row {
                        Some(row) => row.id,
                        None => {
                            report.metadata_rebuilt += 1;
                            self.database
                                .restore_orphan_metadata(
                                    project_id,
                                    &hash,
                                    media_type.as_deref(),
                                    i64::try_from(bytes.len()).unwrap_or(i64::MAX),
                                    "sha256",
                                )
                                .await?
                        }
                    };
                    self.storage
                        .store(project_id, ObjectName::new(&hash, incarnation), &bytes)
                        .await?;
                    incarnation
                };
                // The other incarnations are names no row holds any more - whether or not the row's own was
                // there already: kept, they are copies nothing accounts for or collects.
                for other in stored.into_iter().filter(|other| *other != incarnation) {
                    self.storage
                        .delete(project_id, ObjectName::new(&hash, other))
                        .await?;
                }

                self.database
                    .restore_durable_trace_file(project_id, trace_id, &hash)
                    .await?;
                // A surviving row names the file, so a claim the backup caught mid-collection is void.
                self.database
                    .restore_clear_deletion_claim(project_id, &hash)
                    .await?;
                if !associated.contains(&hash) {
                    report.associations_rebuilt += 1;
                }
                self.database.sync_ref_count(project_id, &hash).await?;
            }
        }

        report.missing_content.sort_by(|left, right| {
            (&left.project_id, &left.trace_id, &left.hash).cmp(&(
                &right.project_id,
                &right.trace_id,
                &right.hash,
            ))
        });
        report.missing_content.dedup();
        Ok(report)
    }
}
