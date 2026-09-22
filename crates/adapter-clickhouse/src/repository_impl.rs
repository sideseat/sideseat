//! Analytics repository port implementation for ClickHouse.
//!
//! This module implements the AnalyticsRepository trait for Arc<ClickhouseService>.
//! ClickHouse operations are natively async so no spawn_blocking needed.

use std::collections::HashMap;
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

#[async_trait]
impl SpanStore for ClickhouseRepository {
    // ==================== Span Operations ====================

    async fn list_spans(&self, params: &ListSpansParams) -> Result<(Vec<SpanRow>, u64), DataError> {
        query::list_spans(self.0.client(), params)
            .await
            .map_err(Into::into)
    }

    async fn get_spans_for_trace(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
    ) -> Result<Vec<SpanRow>, DataError> {
        query::get_spans_for_trace(self.0.client(), project_id, trace_id)
            .await
            .map_err(Into::into)
    }

    async fn get_span(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Option<SpanRow>, DataError> {
        query::get_span(self.0.client(), project_id, trace_id, span_id)
            .await
            .map_err(Into::into)
    }

    async fn get_events_for_span(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Vec<EventRow>, DataError> {
        query::get_events_for_span(self.0.client(), project_id, trace_id, span_id)
            .await
            .map_err(Into::into)
    }

    async fn get_links_for_span(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Vec<LinkRow>, DataError> {
        query::get_links_for_span(self.0.client(), project_id, trace_id, span_id)
            .await
            .map_err(Into::into)
    }

    async fn get_span_counts_bulk(
        &self,
        project_id: &ProjectId,
        span_keys: &[(String, String)],
    ) -> Result<HashMap<(String, String), SpanCounts>, DataError> {
        query::get_span_counts_bulk(self.0.client(), project_id, span_keys)
            .await
            .map_err(Into::into)
    }

    async fn get_feed_spans(&self, params: &FeedSpansParams) -> Result<Vec<SpanRow>, DataError> {
        query::get_feed_spans(self.0.client(), params)
            .await
            .map_err(Into::into)
    }

    async fn get_span_filter_options(
        &self,
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
        observations_only: bool,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        query::get_span_filter_options(
            self.0.client(),
            project_id,
            columns,
            from_timestamp,
            to_timestamp,
            observations_only,
        )
        .await
        .map_err(Into::into)
    }

    async fn delete_spans(
        &self,
        project_id: &ProjectId,
        span_keys: &[(String, String)],
    ) -> Result<u64, DataError> {
        let table = self.0.delete_table("otel_spans");
        let logs_table = self.0.delete_table("otel_logs");
        let on_cluster = self.0.on_cluster_clause();
        query::delete_spans(
            self.0.client(),
            &table,
            &logs_table,
            &on_cluster,
            project_id,
            span_keys,
        )
        .await
        .map_err(Into::into)
    }

    // ==================== Ingestion Operations ====================

    async fn insert_spans(&self, mut spans: Vec<NormalizedSpan>) -> Result<(), DataError> {
        let now = self.0.clock().now();
        for span in &mut spans {
            span.ingested_at.get_or_insert(now);
        }
        // Use local table for distributed mode for optimal insert performance
        let table = self.0.insert_table("otel_spans");
        span::insert_batch(self.0.client(), &table, &spans)
            .await
            .map_err(Into::into)
    }

    async fn spans_match_content(
        &self,
        project_id: &ProjectId,
        records: &[(String, String, String)],
    ) -> Result<bool, DataError> {
        query::spans_match_content(self.0.client(), project_id.as_str(), records)
            .await
            .map_err(Into::into)
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
        // Use local table for distributed mode for optimal insert performance
        let table = self.0.insert_table("otel_metrics");
        metric::insert_batch(self.0.client(), &table, &metrics)
            .await
            .map_err(Into::into)
    }

    async fn list_metrics(
        &self,
        params: &ListMetricsParams,
    ) -> Result<(Vec<MetricRow>, u64), DataError> {
        metric::list_metrics(self.0.client(), params)
            .await
            .map_err(Into::into)
    }

    async fn get_metric(
        &self,
        project_id: &ProjectId,
        datapoint_id: &str,
    ) -> Result<Option<MetricRow>, DataError> {
        metric::get_metric(self.0.client(), project_id, datapoint_id)
            .await
            .map_err(Into::into)
    }

    async fn aggregate_metrics(
        &self,
        params: &ListMetricsParams,
    ) -> Result<Vec<MetricAggregateRow>, DataError> {
        metric::aggregate_metrics(self.0.client(), params)
            .await
            .map_err(Into::into)
    }

    async fn get_metric_filter_options(
        &self,
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        metric::get_metric_filter_options(
            self.0.client(),
            project_id,
            columns,
            from_timestamp,
            to_timestamp,
        )
        .await
        .map_err(Into::into)
    }

    async fn metrics_match_content(
        &self,
        project_id: &ProjectId,
        records: &[(String, String)],
    ) -> Result<bool, DataError> {
        metric::matches_content(self.0.client(), project_id, records)
            .await
            .map_err(Into::into)
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
        log::insert_batch(self.0.client(), &table, &logs)
            .await
            .map_err(Into::into)
    }

    async fn list_logs(&self, params: &ListLogsParams) -> Result<(Vec<LogRow>, u64), DataError> {
        log::list_logs(self.0.client(), params)
            .await
            .map_err(Into::into)
    }

    async fn get_log(
        &self,
        project_id: &ProjectId,
        log_digest: &str,
        ordinal: u32,
    ) -> Result<Option<LogRow>, DataError> {
        log::get_log(self.0.client(), project_id, log_digest, ordinal)
            .await
            .map_err(Into::into)
    }

    async fn get_log_filter_options(
        &self,
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        log::get_log_filter_options(
            self.0.client(),
            project_id,
            columns,
            from_timestamp,
            to_timestamp,
        )
        .await
        .map_err(Into::into)
    }

    async fn logs_match_content(
        &self,
        project_id: &ProjectId,
        records: &[(String, u32)],
    ) -> Result<bool, DataError> {
        log::matches_content(self.0.client(), project_id, records)
            .await
            .map_err(Into::into)
    }
}

#[async_trait]
impl SearchIndex for ClickhouseRepository {
    async fn search(&self, request: &SearchQuery) -> Result<SearchPage, DataError> {
        search::search(self.0.client(), request)
            .await
            .map_err(Into::into)
    }

    async fn search_arrivals_detected(
        &self,
        request: &SearchQuery,
        through: &sideseat_ports::types::SearchCursor,
    ) -> Result<bool, DataError> {
        search::arrivals_detected(self.0.client(), request, through)
            .await
            .map_err(Into::into)
    }

    async fn search_backfill_page(
        &self,
        project_id: &ProjectId,
        signal: SearchSignal,
        limit: usize,
    ) -> Result<Vec<SearchBackfillSource>, DataError> {
        search::backfill_page(self.0.client(), project_id.as_str(), signal, limit)
            .await
            .map_err(Into::into)
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
        search::write_backfill(
            self.0.client(),
            &table,
            &self.0.on_cluster_clause(),
            project_id.as_str(),
            signal,
            documents,
        )
        .await
        .map_err(Into::into)
    }
}

#[async_trait]
impl EntityQuery for ClickhouseRepository {
    // ==================== Trace Operations ====================

    async fn list_traces(
        &self,
        params: &ListTracesParams,
    ) -> Result<(Vec<TraceRow>, u64), DataError> {
        query::list_traces(self.0.client(), params)
            .await
            .map_err(Into::into)
    }

    async fn get_trace(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
    ) -> Result<Option<TraceRow>, DataError> {
        query::get_trace(self.0.client(), project_id, trace_id)
            .await
            .map_err(Into::into)
    }

    async fn get_trace_filter_options(
        &self,
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        query::get_trace_filter_options(
            self.0.client(),
            project_id,
            columns,
            from_timestamp,
            to_timestamp,
        )
        .await
        .map_err(Into::into)
    }

    async fn get_trace_tags_options(
        &self,
        project_id: &ProjectId,
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<Vec<FilterOptionRow>, DataError> {
        query::get_trace_tags_options(self.0.client(), project_id, from_timestamp, to_timestamp)
            .await
            .map_err(Into::into)
    }

    async fn traces_without_spans(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        query::traces_without_spans(self.0.client(), project_id, trace_ids)
            .await
            .map_err(Into::into)
    }

    async fn delete_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<u64, DataError> {
        let table = self.0.delete_table("otel_spans");
        let logs_table = self.0.delete_table("otel_logs");
        let on_cluster = self.0.on_cluster_clause();
        query::delete_traces(
            self.0.client(),
            &table,
            &logs_table,
            &on_cluster,
            project_id,
            trace_ids,
        )
        .await
        .map_err(Into::into)
    }

    // ==================== Session Operations ====================

    async fn list_sessions(
        &self,
        params: &ListSessionsParams,
    ) -> Result<(Vec<SessionRow>, u64), DataError> {
        query::list_sessions(self.0.client(), params)
            .await
            .map_err(Into::into)
    }

    async fn get_session(
        &self,
        project_id: &ProjectId,
        session_id: &str,
    ) -> Result<Option<SessionRow>, DataError> {
        query::get_session(self.0.client(), project_id, session_id)
            .await
            .map_err(Into::into)
    }

    async fn get_traces_for_session(
        &self,
        project_id: &ProjectId,
        session_id: &str,
    ) -> Result<Vec<TraceRow>, DataError> {
        query::get_traces_for_session(self.0.client(), project_id, session_id)
            .await
            .map_err(Into::into)
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
        query::get_trace_session_pairs(self.0.client(), project_id, trace_ids, as_of_us)
            .await
            .map_err(Into::into)
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
        query::get_session_ids_for_traces(self.0.client(), project_id, trace_ids, as_of_us)
            .await
            .map_err(Into::into)
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
        query::get_trace_ids_for_sessions(self.0.client(), project_id, session_ids, as_of_us)
            .await
            .map_err(Into::into)
    }

    async fn get_session_filter_options(
        &self,
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        query::get_session_filter_options(
            self.0.client(),
            project_id,
            columns,
            from_timestamp,
            to_timestamp,
        )
        .await
        .map_err(Into::into)
    }

    async fn delete_sessions(
        &self,
        project_id: &ProjectId,
        session_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        let table = self.0.delete_table("otel_spans");
        let logs_table = self.0.delete_table("otel_logs");
        let on_cluster = self.0.on_cluster_clause();
        query::delete_sessions(
            self.0.client(),
            &table,
            &logs_table,
            &on_cluster,
            project_id,
            session_ids,
        )
        .await
        .map_err(Into::into)
    }

    // ==================== Stats Operations ====================

    async fn get_project_stats(
        &self,
        params: &StatsParams,
    ) -> Result<ProjectStatsResult, DataError> {
        stats::get_project_stats(self.0.client(), params, self.0.clock().now())
            .await
            .map_err(Into::into)
    }
}

#[async_trait]
impl MessageStore for ClickhouseRepository {
    // ==================== Message Operations ====================

    async fn get_messages(
        &self,
        params: &MessageQueryParams,
    ) -> Result<MessageQueryResult, DataError> {
        messages::get_messages(self.0.client(), params)
            .await
            .map_err(Into::into)
    }

    async fn get_project_messages(
        &self,
        params: &FeedMessagesParams,
    ) -> Result<MessageQueryResult, DataError> {
        messages::get_project_messages(self.0.client(), params)
            .await
            .map_err(Into::into)
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
        query::delete_project_data(
            self.0.client(),
            &spans_table,
            &metrics_table,
            &logs_table,
            &on_cluster,
            project_id,
        )
        .await
        .map_err(Into::into)
    }

    async fn count_project_rows(&self, project_id: &ProjectId) -> Result<u64, DataError> {
        let metrics_table = self.0.delete_table("otel_metrics");
        query::count_project_rows(self.0.client(), &metrics_table, project_id)
            .await
            .map_err(Into::into)
    }

    async fn max_ingested_at_us(&self, project_id: &ProjectId) -> Result<Option<i64>, DataError> {
        query::max_ingested_at_us(self.0.client(), project_id.as_str())
            .await
            .map_err(Into::into)
    }

    async fn count_spans_by_project(
        &self,
        project_ids: &[ProjectId],
    ) -> Result<HashMap<String, u64>, DataError> {
        let project_ids = project_ids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        query::count_spans_by_project(self.0.client(), &project_ids)
            .await
            .map_err(Into::into)
    }

    async fn patch_project_hold(
        &self,
        project_id: &ProjectId,
        hold_until: DateTime<Utc>,
    ) -> Result<(), DataError> {
        let spans_table = self.0.delete_table("otel_spans");
        let metrics_table = self.0.delete_table("otel_metrics");
        let logs_table = self.0.delete_table("otel_logs");
        query::patch_project_hold(
            self.0.client(),
            &spans_table,
            &metrics_table,
            &logs_table,
            &self.0.on_cluster_clause(),
            project_id,
            hold_until,
        )
        .await
        .map_err(Into::into)
    }

    async fn project_logical_bytes(&self, project_id: &ProjectId) -> Result<u64, DataError> {
        query::project_logical_bytes(self.0.client(), project_id, None)
            .await
            .map_err(Into::into)
    }

    async fn project_held_logical_bytes(
        &self,
        project_id: &ProjectId,
        now: DateTime<Utc>,
    ) -> Result<u64, DataError> {
        query::project_logical_bytes(self.0.client(), project_id, Some(now))
            .await
            .map_err(Into::into)
    }

    async fn oldest_reclaimable_spans(
        &self,
        project_id: &ProjectId,
        target_bytes: u64,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<PressureSpanCandidate>, DataError> {
        query::oldest_reclaimable_spans(self.0.client(), project_id, target_bytes, now, limit)
            .await
            .map_err(Into::into)
    }
}

#[async_trait]
impl SurvivorReferences for ClickhouseRepository {
    async fn file_reference_fields_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        query::file_reference_fields_for_traces(self.0.client(), project_id, trace_ids)
            .await
            .map_err(Into::into)
    }

    async fn span_body_fields_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<sideseat_ports::types::SpanBodySource>, DataError> {
        query::span_body_fields_for_traces(self.0.client(), project_id, trace_ids)
            .await
            .map_err(Into::into)
    }

    async fn span_body_backfill_page(
        &self,
        project_id: &ProjectId,
        after: Option<(String, String)>,
        limit: usize,
    ) -> Result<Vec<sideseat_ports::types::SpanBodySource>, DataError> {
        query::span_body_backfill_page(self.0.client(), project_id, after, limit)
            .await
            .map_err(Into::into)
    }
}
