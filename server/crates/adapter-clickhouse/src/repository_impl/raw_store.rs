use super::*;
use crate::repositories::raw;
use sideseat_ports::traits::RawStore;
use sideseat_ports::types::{RawPending, RawRecordRow};

fn by_project(records: &[RawRecordRow]) -> BTreeMap<&ProjectId, Vec<RawRecordRow>> {
    let mut grouped = BTreeMap::<&ProjectId, Vec<RawRecordRow>>::new();
    for record in records {
        grouped
            .entry(&record.project_id)
            .or_default()
            .push(record.clone());
    }
    grouped
}

#[async_trait]
impl RawStore for ClickhouseRepository {
    async fn insert_raw_records(&self, records: &[RawRecordRow]) -> Result<(), DataError> {
        for (project_id, records) in by_project(records) {
            let result: Result<(), DataError> =
                tenant_query!(self, project_id, raw::insert, &records);
            result?;
        }
        Ok(())
    }

    /// Appended, then queued: not one step, so a writer that dies between the two leaves a version nothing looks at
    /// again until its time-to-live - part of the open defect the port names.
    async fn append_raw_records(&self, records: &[RawRecordRow]) -> Result<(), DataError> {
        for (project_id, records) in by_project(records) {
            let result: Result<(), DataError> =
                tenant_query!(self, project_id, raw::append, &records);
            result?;
            let raw_ids: Vec<String> = records.iter().map(|record| record.raw_id.clone()).collect();
            self.enqueue_raw_records(project_id, &raw_ids).await?;
        }
        Ok(())
    }

    async fn get_raw_records(
        &self,
        project_id: &ProjectId,
        raw_ids: &[String],
    ) -> Result<Vec<RawRecordRow>, DataError> {
        tenant_query!(self, project_id, raw::get, project_id, raw_ids)
    }

    async fn raw_records_page(
        &self,
        project_id: &ProjectId,
        after: Option<(DateTime<Utc>, String)>,
        limit: usize,
    ) -> Result<Vec<RawRecordRow>, DataError> {
        tenant_query!(self, project_id, raw::page, project_id, after, limit)
    }

    /// Not one step: ClickHouse reads who names the records and then deletes, so a row written in between
    /// loses its record if the caller dies before it looks again - the open defect the port names, until the
    /// transactional store arbitrates.
    async fn delete_unnamed_raw_records(
        &self,
        project_id: &ProjectId,
        raw_ids: &[String],
    ) -> Result<std::collections::HashSet<String>, DataError> {
        let named = self.raw_records_named(project_id, raw_ids).await?;
        let mut unnamed: Vec<String> = raw_ids
            .iter()
            .filter(|raw_id| !named.contains(*raw_id))
            .cloned()
            .collect();
        unnamed.sort_unstable();
        unnamed.dedup();
        if unnamed.is_empty() {
            return Ok(std::collections::HashSet::new());
        }
        let tables = [
            self.0.delete_table("otel_raw"),
            self.0.delete_table("otel_raw_traces"),
        ];
        let on_cluster = self.0.on_cluster_clause();
        let deleted: Result<(), DataError> = tenant_query!(
            self,
            project_id,
            raw::delete,
            &tables,
            &on_cluster,
            project_id,
            &unnamed
        );
        deleted?;
        Ok(unnamed.into_iter().collect())
    }

    /// Not one step either, for the same reason as the delete.
    async fn append_raw_rewrites(
        &self,
        records: &[RawRecordRow],
    ) -> Result<std::collections::HashSet<String>, DataError> {
        let mut appended = std::collections::HashSet::new();
        for (project_id, records) in by_project(records) {
            let ids: Vec<String> = records.iter().map(|record| record.raw_id.clone()).collect();
            let stored: std::collections::HashSet<String> = self
                .get_raw_records(project_id, &ids)
                .await?
                .into_iter()
                .map(|row| row.raw_id)
                .collect();
            let kept: Vec<RawRecordRow> = records
                .into_iter()
                .filter(|record| stored.contains(&record.raw_id))
                .collect();
            if kept.is_empty() {
                continue;
            }
            let result: Result<(), DataError> = tenant_query!(self, project_id, raw::append, &kept);
            result?;
            let raw_ids: Vec<String> = kept.into_iter().map(|record| record.raw_id).collect();
            self.enqueue_raw_records(project_id, &raw_ids).await?;
            appended.extend(raw_ids);
        }
        Ok(appended)
    }

    async fn span_raw_ids(
        &self,
        project_id: &ProjectId,
        spans: &[(String, String)],
    ) -> Result<std::collections::HashMap<(String, String), String>, DataError> {
        tenant_query!(self, project_id, raw::span_raw_ids, project_id, spans)
    }

    async fn raw_records_named(
        &self,
        project_id: &ProjectId,
        raw_ids: &[String],
    ) -> Result<std::collections::HashSet<String>, DataError> {
        tenant_query!(self, project_id, raw::named, project_id, raw_ids)
    }

    async fn enqueue_raw_records(
        &self,
        project_id: &ProjectId,
        raw_ids: &[String],
    ) -> Result<(), DataError> {
        tenant_query!(self, project_id, raw::enqueue, project_id, raw_ids)
    }

    async fn pending_raw_records(&self, limit: usize) -> Result<Vec<RawPending>, DataError> {
        maintenance_query!(self, raw::pending, limit)
    }

    async fn clear_raw_pending(&self, entries: &[RawPending]) -> Result<(), DataError> {
        let table = self.0.delete_table("otel_raw_pending");
        let on_cluster = self.0.on_cluster_clause();
        maintenance_query!(self, raw::clear, &table, &on_cluster, entries)
    }
}
