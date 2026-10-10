use super::*;

#[async_trait]
impl AnalyticsMaintenance for DuckdbRepository {
    fn fatal_failure(&self) -> Option<String> {
        self.0.fatal_failure().map(|failure| failure.to_string())
    }

    // ==================== Project Data Operations ====================

    async fn analytics_project_ids(&self, limit: usize) -> Result<Vec<ProjectId>, DataError> {
        let db = Arc::clone(&self.0);
        DuckdbService::run_query(&self.0, move || {
            let conn = db.conn();
            query::analytics_project_ids(&conn, limit)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn delete_project_data(&self, project_id: &ProjectId) -> Result<u64, DataError> {
        let db = Arc::clone(&self.0);
        let pid = project_id.to_string();
        DuckdbService::run_query(&self.0, move || {
            db.write(|conn| query::delete_project_data(conn, &pid))
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn count_project_rows(&self, project_id: &ProjectId) -> Result<u64, DataError> {
        let db = Arc::clone(&self.0);
        let id = project_id.to_string();
        DuckdbService::run_query(&self.0, move || {
            let conn = db.conn();
            query::count_project_rows(&conn, &id)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn max_ingested_at_us(&self, project_id: &ProjectId) -> Result<Option<i64>, DataError> {
        let db = Arc::clone(&self.0);
        let id = project_id.to_string();
        DuckdbService::run_query(&self.0, move || {
            let conn = db.conn();
            query::max_ingested_at_us(&conn, &id)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn count_spans_by_project(
        &self,
        project_ids: &[ProjectId],
    ) -> Result<HashMap<String, u64>, DataError> {
        let db = Arc::clone(&self.0);
        let ids = project_ids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        DuckdbService::run_query(&self.0, move || {
            let conn = db.conn();
            query::count_spans_by_project(&conn, &ids)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn patch_project_hold(
        &self,
        project_id: &ProjectId,
        hold_until: DateTime<Utc>,
    ) -> Result<(), DataError> {
        let db = Arc::clone(&self.0);
        let id = project_id.to_string();
        DuckdbService::run_query(&self.0, move || {
            db.write(|conn| query::patch_project_hold(conn, &id, hold_until))
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn project_logical_bytes(&self, project_id: &ProjectId) -> Result<u64, DataError> {
        let db = Arc::clone(&self.0);
        let id = project_id.to_string();
        DuckdbService::run_query(&self.0, move || {
            let conn = db.conn();
            query::project_logical_bytes(&conn, &id, None)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn project_held_logical_bytes(
        &self,
        project_id: &ProjectId,
        now: DateTime<Utc>,
    ) -> Result<u64, DataError> {
        let db = Arc::clone(&self.0);
        let id = project_id.to_string();
        DuckdbService::run_query(&self.0, move || {
            let conn = db.conn();
            query::project_logical_bytes(&conn, &id, Some(now))
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn oldest_reclaimable_spans(
        &self,
        project_id: &ProjectId,
        target_bytes: u64,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<PressureSpanCandidate>, DataError> {
        let db = Arc::clone(&self.0);
        let id = project_id.to_string();
        DuckdbService::run_query(&self.0, move || {
            let conn = db.conn();
            query::oldest_reclaimable_spans(&conn, &id, target_bytes, now, limit)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }
}
