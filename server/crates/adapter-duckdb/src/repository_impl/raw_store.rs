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
    let db = Arc::clone(db);
    DuckdbService::run_query(move || call(&db.conn()))
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
        run(&self.0, move |conn| raw::append_versions(conn, &records)).await
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

    async fn delete_raw_records(
        &self,
        project_id: &ProjectId,
        raw_ids: &[String],
    ) -> Result<(), DataError> {
        let (project_id, raw_ids) = (project_id.clone(), raw_ids.to_vec());
        run(&self.0, move |conn| {
            raw::delete(conn, &project_id, &raw_ids)
        })
        .await
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

    async fn raw_records_named(
        &self,
        project_id: &ProjectId,
        raw_ids: &[String],
    ) -> Result<std::collections::HashSet<String>, DataError> {
        let (project_id, raw_ids) = (project_id.clone(), raw_ids.to_vec());
        run(&self.0, move |conn| raw::named(conn, &project_id, &raw_ids)).await
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

    async fn pending_raw_records(&self, limit: usize) -> Result<Vec<RawPending>, DataError> {
        run(&self.0, move |conn| raw::pending(conn, limit)).await
    }

    async fn clear_raw_pending(&self, entries: &[RawPending]) -> Result<(), DataError> {
        let entries = entries.to_vec();
        run(&self.0, move |conn| raw::clear(conn, &entries)).await
    }
}
