//! Analytics repository port implementation for ClickHouse.
//!
//! This module implements the AnalyticsRepository trait for Arc<ClickhouseService>.
//! ClickHouse operations are natively async so no spawn_blocking needed.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use sideseat_ports::error::DataError;
use sideseat_ports::traits::{
    AnalyticsMaintenance, EntityQuery, FilterOptionRow, LogStore, MessageStore, MetricStore,
    SearchIndex, SpanStore, SurvivorReferences,
};
use sideseat_ports::types::{
    EventRow, FeedMessagesParams, FeedSpansParams, LinkRow, ListLogsParams, ListMetricsParams,
    ListSessionsParams, ListSpansParams, ListTracesParams, LogRow, MessageQueryParams,
    MessageQueryResult, MetricAggregateRow, MetricRow, NormalizedLog, NormalizedMetric,
    NormalizedSpan, PressureSpanCandidate, ProjectId, ProjectStatsResult, SearchBackfillDocument,
    SearchBackfillSource, SearchPage, SearchQuery, SearchSignal, SessionRow, SpanCounts, SpanRow,
    StatsParams, TraceRow,
};

use super::ClickhouseService;
use super::repositories::{log, messages, metric, query, search, span, stats};

/// The port, implemented over the service.
///
/// A **wrapper rather than `impl … for Arc<ClickhouseService>`**, and that is the orphan rule rather than taste: with the
/// trait in `sideseat-ports` and `Arc` in `std`, an impl on `Arc<ClickhouseService>` has no local type ahead of an
/// uncovered parameter, so it is refused across a crate boundary. It compiled only while everything was one
/// crate - which is one more way the single crate hid the direction of its own dependencies.
#[derive(Clone)]
pub struct ClickhouseRepository(pub Arc<ClickhouseService>);

/// So the wrapper is transparent to the service's own methods.
///
/// Without this, wrapping turns every call that is *not* a port method - a maintenance helper, a test probe -
/// into `wrapper.0.method()`, which is noise that says nothing. The port methods live on the wrapper itself and
/// are found first, so nothing is shadowed.
impl std::ops::Deref for ClickhouseRepository {
    type Target = ClickhouseService;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

macro_rules! tenant_query {
    ($repository:expr, $project_id:expr, $function:path $(, $argument:expr)* $(,)?) => {{
        let client = $repository.0.tenant_client($project_id);
        $function(&client $(, $argument)*).await.map_err(Into::into)
    }};
}

macro_rules! maintenance_query {
    ($repository:expr, $function:path $(, $argument:expr)* $(,)?) => {{
        let client = $repository.0.maintenance_client();
        $function(&client $(, $argument)*).await.map_err(Into::into)
    }};
}

#[async_trait]
impl SpanStore for ClickhouseRepository {
    // ==================== Span Operations ====================

    async fn list_spans(&self, params: &ListSpansParams) -> Result<(Vec<SpanRow>, u64), DataError> {
        tenant_query!(self, &params.project_id, query::list_spans, params)
    }

    async fn get_spans_for_trace(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
    ) -> Result<Vec<SpanRow>, DataError> {
        tenant_query!(
            self,
            project_id,
            query::get_spans_for_trace,
            project_id,
            trace_id
        )
    }

    async fn get_span(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Option<SpanRow>, DataError> {
        tenant_query!(
            self,
            project_id,
            query::get_span,
            project_id,
            trace_id,
            span_id
        )
    }

    async fn get_events_for_span(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Vec<EventRow>, DataError> {
        tenant_query!(
            self,
            project_id,
            query::get_events_for_span,
            project_id,
            trace_id,
            span_id
        )
    }

    async fn get_links_for_span(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Vec<LinkRow>, DataError> {
        tenant_query!(
            self,
            project_id,
            query::get_links_for_span,
            project_id,
            trace_id,
            span_id
        )
    }

    async fn get_span_counts_bulk(
        &self,
        project_id: &ProjectId,
        span_keys: &[(String, String)],
    ) -> Result<HashMap<(String, String), SpanCounts>, DataError> {
        tenant_query!(
            self,
            project_id,
            query::get_span_counts_bulk,
            project_id,
            span_keys
        )
    }

    async fn get_feed_spans(&self, params: &FeedSpansParams) -> Result<Vec<SpanRow>, DataError> {
        tenant_query!(self, &params.project_id, query::get_feed_spans, params)
    }

    async fn get_span_filter_options(
        &self,
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
        observations_only: bool,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        tenant_query!(
            self,
            project_id,
            query::get_span_filter_options,
            project_id,
            columns,
            from_timestamp,
            to_timestamp,
            observations_only,
        )
    }

    async fn delete_spans(
        &self,
        project_id: &ProjectId,
        span_keys: &[(String, String)],
    ) -> Result<u64, DataError> {
        let table = self.0.delete_table("otel_spans");
        let logs_table = self.0.delete_table("otel_logs");
        let on_cluster = self.0.on_cluster_clause();
        tenant_query!(
            self,
            project_id,
            query::delete_spans,
            &table,
            &logs_table,
            &on_cluster,
            project_id,
            span_keys,
        )
    }

    // ==================== Ingestion Operations ====================

    async fn insert_spans(&self, mut spans: Vec<NormalizedSpan>) -> Result<(), DataError> {
        let now = self.0.clock().now();
        for span in &mut spans {
            span.ingested_at.get_or_insert(now);
        }
        let table = self.0.insert_table("otel_spans");
        let mut by_project = BTreeMap::<String, Vec<NormalizedSpan>>::new();
        for span in spans {
            let project_id = span
                .project_id
                .as_deref()
                .filter(|project_id| !project_id.is_empty())
                .ok_or_else(|| DataError::Conflict("span has no project_id".to_owned()))?;
            by_project
                .entry(project_id.to_owned())
                .or_default()
                .push(span);
        }
        for (project_id, spans) in by_project {
            let client = self.0.tenant_client_str(&project_id);
            span::insert_batch(&client, &table, &spans)
                .await
                .map_err(DataError::from)?;
        }
        Ok(())
    }

    async fn spans_match_content(
        &self,
        project_id: &ProjectId,
        records: &[(String, String, String)],
    ) -> Result<bool, DataError> {
        tenant_query!(
            self,
            project_id,
            query::spans_match_content,
            project_id.as_str(),
            records
        )
    }
}

#[async_trait]
impl MetricStore for ClickhouseRepository {
    async fn insert_metrics(&self, metrics: &[NormalizedMetric]) -> Result<(), DataError> {
        let now = self.0.clock().now();
        let mut metrics = metrics.to_vec();
        for metric in &mut metrics {
            metric.ingested_at.get_or_insert(now);
        }
        let table = self.0.insert_table("otel_metrics");
        let mut by_project = BTreeMap::<String, Vec<NormalizedMetric>>::new();
        for metric in metrics {
            let project_id = metric
                .project_id
                .as_deref()
                .filter(|project_id| !project_id.is_empty())
                .ok_or_else(|| DataError::Conflict("metric has no project_id".to_owned()))?;
            by_project
                .entry(project_id.to_owned())
                .or_default()
                .push(metric);
        }
        for (project_id, metrics) in by_project {
            let client = self.0.tenant_client_str(&project_id);
            metric::insert_batch(&client, &table, &metrics)
                .await
                .map_err(DataError::from)?;
        }
        Ok(())
    }

    async fn list_metrics(
        &self,
        params: &ListMetricsParams,
    ) -> Result<(Vec<MetricRow>, u64), DataError> {
        tenant_query!(self, &params.project_id, metric::list_metrics, params)
    }

    async fn get_metric(
        &self,
        project_id: &ProjectId,
        datapoint_id: &str,
    ) -> Result<Option<MetricRow>, DataError> {
        tenant_query!(
            self,
            project_id,
            metric::get_metric,
            project_id,
            datapoint_id
        )
    }

    async fn aggregate_metrics(
        &self,
        params: &ListMetricsParams,
    ) -> Result<Vec<MetricAggregateRow>, DataError> {
        tenant_query!(self, &params.project_id, metric::aggregate_metrics, params)
    }

    async fn get_metric_filter_options(
        &self,
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        tenant_query!(
            self,
            project_id,
            metric::get_metric_filter_options,
            project_id,
            columns,
            from_timestamp,
            to_timestamp,
        )
    }

    async fn metrics_match_content(
        &self,
        project_id: &ProjectId,
        records: &[(String, String)],
    ) -> Result<bool, DataError> {
        tenant_query!(
            self,
            project_id,
            metric::matches_content,
            project_id,
            records
        )
    }
}

#[async_trait]
impl LogStore for ClickhouseRepository {
    async fn insert_logs(&self, logs: &[NormalizedLog]) -> Result<(), DataError> {
        let now = self.0.clock().now();
        let mut logs = logs.to_vec();
        for log in &mut logs {
            log.ingested_at.get_or_insert(now);
        }
        let table = self.0.insert_table("otel_logs");
        let mut by_project = BTreeMap::<String, Vec<NormalizedLog>>::new();
        for log in logs {
            let project_id = log
                .project_id
                .as_deref()
                .filter(|project_id| !project_id.is_empty())
                .ok_or_else(|| DataError::Conflict("log has no project_id".to_owned()))?;
            by_project
                .entry(project_id.to_owned())
                .or_default()
                .push(log);
        }
        for (project_id, logs) in by_project {
            let client = self.0.tenant_client_str(&project_id);
            log::insert_batch(&client, &table, &logs)
                .await
                .map_err(DataError::from)?;
        }
        Ok(())
    }

    async fn list_logs(&self, params: &ListLogsParams) -> Result<(Vec<LogRow>, u64), DataError> {
        tenant_query!(self, &params.project_id, log::list_logs, params)
    }

    async fn get_log(
        &self,
        project_id: &ProjectId,
        log_digest: &str,
        ordinal: u32,
    ) -> Result<Option<LogRow>, DataError> {
        tenant_query!(
            self,
            project_id,
            log::get_log,
            project_id,
            log_digest,
            ordinal
        )
    }

    async fn get_log_filter_options(
        &self,
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        tenant_query!(
            self,
            project_id,
            log::get_log_filter_options,
            project_id,
            columns,
            from_timestamp,
            to_timestamp,
        )
    }

    async fn logs_match_content(
        &self,
        project_id: &ProjectId,
        records: &[(String, u32)],
    ) -> Result<bool, DataError> {
        tenant_query!(self, project_id, log::matches_content, project_id, records)
    }
}

#[async_trait]
impl SearchIndex for ClickhouseRepository {
    async fn search(&self, request: &SearchQuery) -> Result<SearchPage, DataError> {
        tenant_query!(self, &request.project_id, search::search, request)
    }

    async fn search_arrivals_detected(
        &self,
        request: &SearchQuery,
        through: &sideseat_ports::types::SearchCursor,
    ) -> Result<bool, DataError> {
        tenant_query!(
            self,
            &request.project_id,
            search::arrivals_detected,
            request,
            through
        )
    }

    async fn search_backfill_page(
        &self,
        project_id: &ProjectId,
        signal: SearchSignal,
        limit: usize,
    ) -> Result<Vec<SearchBackfillSource>, DataError> {
        tenant_query!(
            self,
            project_id,
            search::backfill_page,
            project_id.as_str(),
            signal,
            limit
        )
    }

    async fn write_search_backfill(
        &self,
        project_id: &ProjectId,
        signal: SearchSignal,
        documents: &[SearchBackfillDocument],
    ) -> Result<(), DataError> {
        let table = match signal {
            SearchSignal::Spans => self.0.delete_table("otel_spans"),
            SearchSignal::Logs => self.0.delete_table("otel_logs"),
        };
        tenant_query!(
            self,
            project_id,
            search::write_backfill,
            &table,
            &self.0.on_cluster_clause(),
            project_id.as_str(),
            signal,
            documents,
        )
    }
}

#[async_trait]
impl EntityQuery for ClickhouseRepository {
    // ==================== Trace Operations ====================

    async fn list_traces(
        &self,
        params: &ListTracesParams,
    ) -> Result<(Vec<TraceRow>, u64), DataError> {
        tenant_query!(self, &params.project_id, query::list_traces, params)
    }

    async fn get_trace(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
    ) -> Result<Option<TraceRow>, DataError> {
        tenant_query!(self, project_id, query::get_trace, project_id, trace_id)
    }

    async fn get_trace_filter_options(
        &self,
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        tenant_query!(
            self,
            project_id,
            query::get_trace_filter_options,
            project_id,
            columns,
            from_timestamp,
            to_timestamp,
        )
    }

    async fn get_trace_tags_options(
        &self,
        project_id: &ProjectId,
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<Vec<FilterOptionRow>, DataError> {
        tenant_query!(
            self,
            project_id,
            query::get_trace_tags_options,
            project_id,
            from_timestamp,
            to_timestamp
        )
    }

    async fn traces_without_spans(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        tenant_query!(
            self,
            project_id,
            query::traces_without_spans,
            project_id,
            trace_ids
        )
    }

    async fn delete_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<u64, DataError> {
        let table = self.0.delete_table("otel_spans");
        let logs_table = self.0.delete_table("otel_logs");
        let on_cluster = self.0.on_cluster_clause();
        tenant_query!(
            self,
            project_id,
            query::delete_traces,
            &table,
            &logs_table,
            &on_cluster,
            project_id,
            trace_ids,
        )
    }

    // ==================== Session Operations ====================

    async fn list_sessions(
        &self,
        params: &ListSessionsParams,
    ) -> Result<(Vec<SessionRow>, u64), DataError> {
        tenant_query!(self, &params.project_id, query::list_sessions, params)
    }

    async fn get_session(
        &self,
        project_id: &ProjectId,
        session_id: &str,
    ) -> Result<Option<SessionRow>, DataError> {
        tenant_query!(self, project_id, query::get_session, project_id, session_id)
    }

    async fn get_traces_for_session(
        &self,
        project_id: &ProjectId,
        session_id: &str,
    ) -> Result<Vec<TraceRow>, DataError> {
        tenant_query!(
            self,
            project_id,
            query::get_traces_for_session,
            project_id,
            session_id
        )
    }

    async fn get_trace_session_pairs(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
        // Honoured, through `ch_membership_source`. It used to be accepted and ignored on the grounds
        // that `FINAL` has no "as of" form - which stopped being true when the message rows got one, and
        // meanwhile a traversal read watermark-era rows with current membership, so it was not a view of
        // one instant. The residual is the engine's: exact only while the pre-watermark version survives
        // unmerged.
        as_of_us: Option<i64>,
    ) -> Result<Vec<(String, String)>, DataError> {
        tenant_query!(
            self,
            project_id,
            query::get_trace_session_pairs,
            project_id,
            trace_ids,
            as_of_us
        )
    }

    async fn get_session_ids_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
        // Honoured, through `ch_membership_source`. It used to be accepted and ignored on the grounds
        // that `FINAL` has no "as of" form - which stopped being true when the message rows got one, and
        // meanwhile a traversal read watermark-era rows with current membership, so it was not a view of
        // one instant. The residual is the engine's: exact only while the pre-watermark version survives
        // unmerged.
        as_of_us: Option<i64>,
    ) -> Result<Vec<String>, DataError> {
        tenant_query!(
            self,
            project_id,
            query::get_session_ids_for_traces,
            project_id,
            trace_ids,
            as_of_us
        )
    }

    async fn get_trace_ids_for_sessions(
        &self,
        project_id: &ProjectId,
        session_ids: &[String],
        // Honoured, through `ch_membership_source`. It used to be accepted and ignored on the grounds
        // that `FINAL` has no "as of" form - which stopped being true when the message rows got one, and
        // meanwhile a traversal read watermark-era rows with current membership, so it was not a view of
        // one instant. The residual is the engine's: exact only while the pre-watermark version survives
        // unmerged.
        as_of_us: Option<i64>,
    ) -> Result<Vec<String>, DataError> {
        tenant_query!(
            self,
            project_id,
            query::get_trace_ids_for_sessions,
            project_id,
            session_ids,
            as_of_us
        )
    }

    async fn get_session_filter_options(
        &self,
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        tenant_query!(
            self,
            project_id,
            query::get_session_filter_options,
            project_id,
            columns,
            from_timestamp,
            to_timestamp,
        )
    }

    async fn delete_sessions(
        &self,
        project_id: &ProjectId,
        session_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        let table = self.0.delete_table("otel_spans");
        let logs_table = self.0.delete_table("otel_logs");
        let on_cluster = self.0.on_cluster_clause();
        tenant_query!(
            self,
            project_id,
            query::delete_sessions,
            &table,
            &logs_table,
            &on_cluster,
            project_id,
            session_ids,
        )
    }

    // ==================== Stats Operations ====================

    async fn get_project_stats(
        &self,
        params: &StatsParams,
    ) -> Result<ProjectStatsResult, DataError> {
        tenant_query!(
            self,
            &params.project_id,
            stats::get_project_stats,
            params,
            self.0.clock().now()
        )
    }
}

#[async_trait]
impl MessageStore for ClickhouseRepository {
    // ==================== Message Operations ====================

    async fn get_messages(
        &self,
        params: &MessageQueryParams,
    ) -> Result<MessageQueryResult, DataError> {
        tenant_query!(self, &params.project_id, messages::get_messages, params)
    }

    async fn get_project_messages(
        &self,
        params: &FeedMessagesParams,
    ) -> Result<MessageQueryResult, DataError> {
        tenant_query!(
            self,
            &params.project_id,
            messages::get_project_messages,
            params
        )
    }
}

#[async_trait]
impl AnalyticsMaintenance for ClickhouseRepository {
    // ==================== Project Data Operations ====================

    async fn delete_project_data(&self, project_id: &ProjectId) -> Result<u64, DataError> {
        let spans_table = self.0.delete_table("otel_spans");
        let metrics_table = self.0.delete_table("otel_metrics");
        let logs_table = self.0.delete_table("otel_logs");
        let on_cluster = self.0.on_cluster_clause();
        tenant_query!(
            self,
            project_id,
            query::delete_project_data,
            &spans_table,
            &metrics_table,
            &logs_table,
            &on_cluster,
            project_id,
        )
    }

    async fn count_project_rows(&self, project_id: &ProjectId) -> Result<u64, DataError> {
        let metrics_table = self.0.delete_table("otel_metrics");
        tenant_query!(
            self,
            project_id,
            query::count_project_rows,
            &metrics_table,
            project_id
        )
    }

    async fn max_ingested_at_us(&self, project_id: &ProjectId) -> Result<Option<i64>, DataError> {
        tenant_query!(
            self,
            project_id,
            query::max_ingested_at_us,
            project_id.as_str()
        )
    }

    async fn count_spans_by_project(
        &self,
        project_ids: &[ProjectId],
    ) -> Result<HashMap<String, u64>, DataError> {
        let project_ids = project_ids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        maintenance_query!(self, query::count_spans_by_project, &project_ids)
    }

    async fn patch_project_hold(
        &self,
        project_id: &ProjectId,
        hold_until: DateTime<Utc>,
    ) -> Result<(), DataError> {
        let spans_table = self.0.delete_table("otel_spans");
        let metrics_table = self.0.delete_table("otel_metrics");
        let logs_table = self.0.delete_table("otel_logs");
        tenant_query!(
            self,
            project_id,
            query::patch_project_hold,
            &spans_table,
            &metrics_table,
            &logs_table,
            &self.0.on_cluster_clause(),
            project_id,
            hold_until,
        )
    }

    async fn project_logical_bytes(&self, project_id: &ProjectId) -> Result<u64, DataError> {
        tenant_query!(
            self,
            project_id,
            query::project_logical_bytes,
            project_id,
            None
        )
    }

    async fn project_held_logical_bytes(
        &self,
        project_id: &ProjectId,
        now: DateTime<Utc>,
    ) -> Result<u64, DataError> {
        tenant_query!(
            self,
            project_id,
            query::project_logical_bytes,
            project_id,
            Some(now)
        )
    }

    async fn oldest_reclaimable_spans(
        &self,
        project_id: &ProjectId,
        target_bytes: u64,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<PressureSpanCandidate>, DataError> {
        tenant_query!(
            self,
            project_id,
            query::oldest_reclaimable_spans,
            project_id,
            target_bytes,
            now,
            limit
        )
    }
}

#[async_trait]
impl SurvivorReferences for ClickhouseRepository {
    async fn file_reference_fields_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        tenant_query!(
            self,
            project_id,
            query::file_reference_fields_for_traces,
            project_id,
            trace_ids
        )
    }

    async fn span_body_fields_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<sideseat_ports::types::SpanBodySource>, DataError> {
        tenant_query!(
            self,
            project_id,
            query::span_body_fields_for_traces,
            project_id,
            trace_ids
        )
    }

    async fn span_body_backfill_page(
        &self,
        project_id: &ProjectId,
        after: Option<(String, String)>,
        limit: usize,
    ) -> Result<Vec<sideseat_ports::types::SpanBodySource>, DataError> {
        tenant_query!(
            self,
            project_id,
            query::span_body_backfill_page,
            project_id,
            after,
            limit
        )
    }
}
