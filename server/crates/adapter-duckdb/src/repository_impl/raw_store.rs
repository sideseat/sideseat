use super::*;
use crate::repositories::raw;
use sideseat_ports::traits::RawStore;
use sideseat_ports::types::{RawPending, RawRecordRow};

/// Run one raw-store call on the DuckDB worker pool.
async fn run<T, F>(db: &Arc<DuckdbService>, call: F) -> Result<T, DataError>
where
    T: Send + 'static,
    F: FnOnce(&duckdb::Connection) -> Result<T, crate::DuckdbError> + Send + 'static,
{
    let service = Arc::clone(db);
    // Through `write` for reads too: a read never creates a WAL, so for it the check is one stat.
    db.run_query(move || service.write(call))
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
}

#[async_trait]
impl RawStore for DuckdbRepository {
    async fn insert_raw_records(&self, records: &[RawRecordRow]) -> Result<(), DataError> {
        let records = records.to_vec();
        run(&self.0, move |conn| raw::insert(conn, &records)).await
    }

    async fn append_raw_records(&self, records: &[RawRecordRow]) -> Result<(), DataError> {
        let records = records.to_vec();
        run(&self.0, move |conn| raw::append_queued(conn, &records)).await
    }

    async fn get_raw_records(
        &self,
        project_id: &ProjectId,
        raw_ids: &[String],
    ) -> Result<Vec<RawRecordRow>, DataError> {
        let (project_id, raw_ids) = (project_id.clone(), raw_ids.to_vec());
        run(&self.0, move |conn| raw::get(conn, &project_id, &raw_ids)).await
    }

    async fn raw_records_page(
        &self,
        project_id: &ProjectId,
        after: Option<(DateTime<Utc>, String)>,
        limit: usize,
    ) -> Result<Vec<RawRecordRow>, DataError> {
        let project_id = project_id.clone();
        run(&self.0, move |conn| {
            raw::page(conn, &project_id, after, limit)
        })
        .await
    }

    fn rewrites_queue_atomically(&self) -> bool {
        // One transaction decides, appends and queues (`raw::append_rewrites`).
        true
    }

    async fn append_raw_rewrites(
        &self,
        rewrites: &[sideseat_ports::types::RawRewrite],
    ) -> Result<std::collections::HashSet<String>, DataError> {
        let rewrites = rewrites.to_vec();
        run(&self.0, move |conn| raw::append_rewrites(conn, &rewrites)).await
    }

    async fn span_raw_ids(
        &self,
        project_id: &ProjectId,
        spans: &[(String, String)],
    ) -> Result<std::collections::HashMap<(String, String), String>, DataError> {
        let (project_id, spans) = (project_id.clone(), spans.to_vec());
        run(&self.0, move |conn| {
            raw::span_raw_ids(conn, &project_id, &spans)
        })
        .await
    }

    async fn enqueue_raw_records(
        &self,
        project_id: &ProjectId,
        raw_ids: &[String],
    ) -> Result<(), DataError> {
        let (project_id, raw_ids) = (project_id.clone(), raw_ids.to_vec());
        run(&self.0, move |conn| {
            raw::enqueue(conn, &project_id, &raw_ids)
        })
        .await
    }

    async fn pending_raw_records(
        &self,
        limit: usize,
        enqueued_before: Option<DateTime<Utc>>,
    ) -> Result<Vec<RawPending>, DataError> {
        run(&self.0, move |conn| {
            raw::pending(conn, limit, enqueued_before)
        })
        .await
    }

    async fn purge_raw_versions(
        &self,
        project_id: &ProjectId,
        raw_id: &str,
        up_to_epoch: i64,
    ) -> Result<(), DataError> {
        let (project_id, raw_id) = (project_id.clone(), raw_id.to_string());
        run(&self.0, move |conn| {
            raw::purge_versions(conn, &project_id, &raw_id, up_to_epoch)
        })
        .await
    }

    async fn delete_superseded_raw_versions(
        &self,
        project_id: &ProjectId,
        raw_id: &str,
        below_version: i64,
    ) -> Result<(), DataError> {
        let (project_id, raw_id) = (project_id.clone(), raw_id.to_string());
        run(&self.0, move |conn| {
            raw::delete_superseded(conn, &project_id, &raw_id, below_version)
        })
        .await
    }

    async fn raw_records_named_in_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<std::collections::HashSet<(String, String)>, DataError> {
        let (project_id, trace_ids) = (project_id.clone(), trace_ids.to_vec());
        run(&self.0, move |conn| {
            raw::named_in_traces(conn, &project_id, &trace_ids)
        })
        .await
    }

    async fn traces_indexed_under_raw_record(
        &self,
        project_id: &ProjectId,
        raw_id: &str,
    ) -> Result<Vec<String>, DataError> {
        let (project_id, raw_id) = (project_id.clone(), raw_id.to_string());
        run(&self.0, move |conn| {
            raw::traces_indexed_under(conn, &project_id, &raw_id)
        })
        .await
    }

    async fn raw_records_indexed_under_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        let (project_id, trace_ids) = (project_id.clone(), trace_ids.to_vec());
        run(&self.0, move |conn| {
            raw::indexed_under_traces(conn, &project_id, &trace_ids)
        })
        .await
    }

    async fn clear_raw_pending(&self, entries: &[RawPending]) -> Result<(), DataError> {
        let entries = entries.to_vec();
        run(&self.0, move |conn| raw::clear(conn, &entries)).await
    }
}
