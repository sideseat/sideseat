use super::*;
use crate::repositories::raw;
use sideseat_ports::traits::RawStore;
use sideseat_ports::types::RawRecordRow;

#[async_trait]
impl RawStore for DuckdbRepository {
    async fn insert_raw_records(&self, records: &[RawRecordRow]) -> Result<(), DataError> {
        let db = Arc::clone(&self.0);
        let records = records.to_vec();
        DuckdbService::run_query(move || raw::insert(&db.conn(), &records))
            .await
            .map_err(DataError::from)?
            .map_err(Into::into)
    }

    async fn get_raw_records(
        &self,
        project_id: &ProjectId,
        raw_ids: &[String],
    ) -> Result<Vec<RawRecordRow>, DataError> {
        let db = Arc::clone(&self.0);
        let project_id = project_id.clone();
        let raw_ids = raw_ids.to_vec();
        DuckdbService::run_query(move || raw::get(&db.conn(), &project_id, &raw_ids))
            .await
            .map_err(DataError::from)?
            .map_err(Into::into)
    }

    async fn raw_records_page(
        &self,
        project_id: &ProjectId,
        after: Option<(DateTime<Utc>, String)>,
        limit: usize,
    ) -> Result<Vec<RawRecordRow>, DataError> {
        let db = Arc::clone(&self.0);
        let project_id = project_id.clone();
        DuckdbService::run_query(move || raw::page(&db.conn(), &project_id, after, limit))
            .await
            .map_err(DataError::from)?
            .map_err(Into::into)
    }

    async fn rewrite_raw_record(
        &self,
        project_id: &ProjectId,
        raw_id: &str,
        record: &[u8],
    ) -> Result<(), DataError> {
        let db = Arc::clone(&self.0);
        let project_id = project_id.clone();
        let raw_id = raw_id.to_string();
        let record = record.to_vec();
        DuckdbService::run_query(move || raw::rewrite(&db.conn(), &project_id, &raw_id, &record))
            .await
            .map_err(DataError::from)?
            .map_err(Into::into)
    }

    async fn delete_unreferenced_raw_records(
        &self,
        project_id: &ProjectId,
        received_before: DateTime<Utc>,
    ) -> Result<u64, DataError> {
        let db = Arc::clone(&self.0);
        let project_id = project_id.clone();
        DuckdbService::run_query(move || {
            raw::delete_unreferenced(&db.conn(), &project_id, received_before)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }
}
