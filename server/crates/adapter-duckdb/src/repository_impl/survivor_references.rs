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
        DuckdbService::run_query(&self.0, move || {
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
        DuckdbService::run_query(&self.0, move || {
            let conn = db.conn();
            query::file_reference_fields_for_traces(&conn, &pid, &tids)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn stored_trace_ids_after(
        &self,
        project_id: &ProjectId,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>, DataError> {
        let db = Arc::clone(&self.0);
        let pid = project_id.to_string();
        let after = after.map(str::to_owned);
        DuckdbService::run_query(&self.0, move || {
            query::stored_trace_ids_after(&db.conn(), &pid, after.as_deref(), limit)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn media_log_trace_ids(
        &self,
        project_id: &ProjectId,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>, DataError> {
        let db = Arc::clone(&self.0);
        let query = sideseat_query_sql::log_media::media_trace_page(
            project_id.as_str(),
            after,
            limit,
            sideseat_query_sql::Backend::Duckdb,
            "log_media_refs",
        );
        DuckdbService::run_query(&self.0, move || {
            let conn = db.conn();
            query::string_column(&conn, &query)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }
}
