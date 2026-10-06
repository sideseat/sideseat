use super::*;
use crate::repositories::raw;
use sideseat_ports::traits::RawStore;
use sideseat_ports::types::RawRecordRow;

#[async_trait]
impl RawStore for ClickhouseRepository {
    async fn insert_raw_records(&self, records: &[RawRecordRow]) -> Result<(), DataError> {
        let mut by_project = BTreeMap::<&ProjectId, Vec<RawRecordRow>>::new();
        for record in records {
            by_project
                .entry(&record.project_id)
                .or_default()
                .push(record.clone());
        }
        for (project_id, records) in by_project {
            let result: Result<(), DataError> =
                tenant_query!(self, project_id, raw::insert, &records);
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

    async fn rewrite_raw_record(
        &self,
        project_id: &ProjectId,
        raw_id: &str,
        record: &[u8],
    ) -> Result<(), DataError> {
        tenant_query!(self, project_id, raw::rewrite, project_id, raw_id, record)
    }

    async fn delete_unreferenced_raw_records(
        &self,
        project_id: &ProjectId,
        received_before: DateTime<Utc>,
    ) -> Result<u64, DataError> {
        tenant_query!(
            self,
            project_id,
            raw::delete_unreferenced,
            project_id,
            received_before
        )
    }
}
