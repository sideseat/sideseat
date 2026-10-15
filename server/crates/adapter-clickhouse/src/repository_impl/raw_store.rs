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
    /// Not one step either, for the same reason as the delete.
    fn rewrites_queue_atomically(&self) -> bool {
        // It reads, then appends, then queues: not one step, until the landing fence.
        false
    }

    async fn append_raw_rewrites(
        &self,
        rewrites: &[sideseat_ports::types::RawRewrite],
    ) -> Result<std::collections::HashSet<String>, DataError> {
        let mut appended = std::collections::HashSet::new();
        // One at a time, each judged against the latest as the ones before it left it, as DuckDB admits them.
        for rewrite in rewrites {
            let row = &rewrite.row;
            // Only while the version it was prepared from is still the latest, as DuckDB decides it. Advisory here:
            // read and then written, and read on one replica, so a version landing in between, or committed on a
            // replica this read does not see, is not seen - the gap the landing fence closes.
            let latest = self
                .get_raw_records(&row.project_id, std::slice::from_ref(&row.raw_id))
                .await?;
            if !latest
                .first()
                .is_some_and(|latest| rewrite.based_on_latest(latest))
            {
                continue;
            }
            let result: Result<(), DataError> = tenant_query!(
                self,
                &row.project_id,
                raw::append,
                std::slice::from_ref(row)
            );
            result?;
            self.enqueue_raw_records(&row.project_id, std::slice::from_ref(&row.raw_id))
                .await?;
            appended.insert(row.raw_id.clone());
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

    async fn enqueue_raw_records(
        &self,
        project_id: &ProjectId,
        raw_ids: &[String],
    ) -> Result<(), DataError> {
        tenant_query!(self, project_id, raw::enqueue, project_id, raw_ids)
    }

    async fn pending_raw_records(
        &self,
        limit: usize,
        enqueued_before: Option<DateTime<Utc>>,
    ) -> Result<Vec<RawPending>, DataError> {
        maintenance_query!(self, raw::pending, limit, enqueued_before)
    }

    async fn purge_raw_versions(
        &self,
        project_id: &ProjectId,
        raw_id: &str,
        up_to_epoch: i64,
    ) -> Result<(), DataError> {
        let table = self.0.delete_table("otel_raw");
        let on_cluster = self.0.on_cluster_clause();
        tenant_query!(
            self,
            project_id,
            raw::purge_versions,
            &table,
            &on_cluster,
            project_id,
            raw_id,
            up_to_epoch
        )
    }

    async fn delete_superseded_raw_versions(
        &self,
        project_id: &ProjectId,
        raw_id: &str,
        below_version: i64,
    ) -> Result<(), DataError> {
        let table = self.0.delete_table("otel_raw");
        let on_cluster = self.0.on_cluster_clause();
        tenant_query!(
            self,
            project_id,
            raw::delete_superseded,
            &table,
            &on_cluster,
            project_id,
            raw_id,
            below_version
        )
    }

    async fn raw_records_named_in_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<std::collections::HashSet<(String, String)>, DataError> {
        let client = self.0.tenant_client(project_id);
        let spans = self.0.every_replica_spans();
        raw::named_in_traces(&client, &spans, project_id, trace_ids)
            .await
            .map_err(Into::into)
    }

    async fn traces_indexed_under_raw_record(
        &self,
        project_id: &ProjectId,
        raw_id: &str,
    ) -> Result<Vec<String>, DataError> {
        // On every replica, as the reads that release are: a trace a lagging replica's index misses would keep
        // its files.
        let traces = self.0.every_replica_table("otel_raw_traces");
        tenant_query!(
            self,
            project_id,
            raw::traces_indexed_under,
            &traces,
            project_id,
            raw_id
        )
    }

    async fn raw_records_indexed_under_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        // On every replica: a retention release enqueues through this, and a record a lagging replica's index
        // misses would go unqueued once its owners are released.
        let traces = self.0.every_replica_table("otel_raw_traces");
        tenant_query!(
            self,
            project_id,
            raw::indexed_under_traces,
            &traces,
            project_id,
            trace_ids
        )
    }

    async fn clear_raw_pending(&self, entries: &[RawPending]) -> Result<(), DataError> {
        let table = self.0.delete_table("otel_raw_pending");
        let on_cluster = self.0.on_cluster_clause();
        maintenance_query!(self, raw::clear, &table, &on_cluster, entries)
    }
}
