use super::*;

#[async_trait]
impl SurvivorReferences for DuckdbRepository {
    async fn survivor_raw_records(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<Vec<u8>>, DataError> {
        let db = Arc::clone(&self.0);
        let pid = project_id.clone();
        let tids = trace_ids.to_vec();
        DuckdbService::run_query(move || {
            crate::repositories::raw::survivor_records(&db.conn(), &pid, &tids)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn file_reference_fields_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        let db = Arc::clone(&self.0);
        let pid = project_id.to_string();
        let tids = trace_ids.to_vec();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::file_reference_fields_for_traces(&conn, &pid, &tids)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn span_body_fields_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<sideseat_ports::types::SpanBodySource>, DataError> {
        let db = Arc::clone(&self.0);
        let pid = project_id.to_string();
        let tids = trace_ids.to_vec();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::span_body_fields_for_traces(&conn, &pid, &tids)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn span_body_backfill_page(
        &self,
        project_id: &ProjectId,
        after: Option<(String, String)>,
        limit: usize,
    ) -> Result<Vec<sideseat_ports::types::SpanBodySource>, DataError> {
        let db = Arc::clone(&self.0);
        let pid = project_id.to_string();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::span_body_backfill_page(&conn, &pid, after, limit)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }
}
