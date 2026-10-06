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

    async fn append_raw_records(&self, records: &[RawRecordRow]) -> Result<(), DataError> {
        for (project_id, records) in by_project(records) {
            let result: Result<(), DataError> =
                tenant_query!(self, project_id, raw::append, &records);
            result?;
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

    async fn delete_raw_records(
        &self,
        project_id: &ProjectId,
        raw_ids: &[String],
    ) -> Result<(), DataError> {
        let tables = [
            self.0.delete_table("otel_raw"),
            self.0.delete_table("otel_raw_traces"),
        ];
        let on_cluster = self.0.on_cluster_clause();
        tenant_query!(
            self,
            project_id,
            raw::delete,
            &tables,
            &on_cluster,
            project_id,
            raw_ids
        )
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
