//! What a survivor release reads: every replica of every shard, so a span whose write returned before the read
//! is in its answer whatever the replication lag.

use super::*;
use sideseat_ports::traits::SurvivorReferences;

#[async_trait]
impl SurvivorReferences for ClickhouseRepository {
    async fn survivor_raw_records(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<Vec<u8>>, DataError> {
        let spans = self.0.every_replica_spans();
        let raw = self.0.every_replica_table("otel_raw");
        tenant_query!(
            self,
            project_id,
            crate::repositories::raw::survivor_records,
            &spans,
            &raw,
            project_id,
            trace_ids
        )
    }

    async fn file_reference_fields_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        let spans = self.0.every_replica_spans();
        tenant_query!(
            self,
            project_id,
            query::file_reference_fields_for_traces,
            &spans,
            project_id,
            trace_ids,
            &self.0.every_replica_table("otel_logs")
        )
    }

    async fn media_log_trace_ids(
        &self,
        project_id: &ProjectId,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>, DataError> {
        tenant_query!(
            self,
            project_id,
            query::media_log_trace_ids,
            project_id.as_str(),
            after,
            limit,
            &self.0.every_replica_table("otel_logs")
        )
    }

    async fn stored_trace_ids_after(
        &self,
        project_id: &ProjectId,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>, DataError> {
        let spans = self.0.every_replica_spans();
        tenant_query!(
            self,
            project_id,
            query::stored_trace_ids_after,
            &spans,
            project_id.as_str(),
            after,
            limit
        )
    }
}
