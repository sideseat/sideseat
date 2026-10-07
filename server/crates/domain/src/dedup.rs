//! Driver-independent deduplication wrapper for `AnalyticsRepository`.
//!
//! Wraps any AnalyticsRepository and deduplicates SpanRow/MessageSpanRow
//! results in Rust. Aggregation queries use SQL-level dedup directly.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};

use sideseat_ports::error::DataError;
use sideseat_ports::traits::{
    AnalyticsMaintenance, AnalyticsRepository, EntityQuery, FilterOptionRow, LogStore,
    MessageStore, MetricStore, SearchIndex, SpanStore, SurvivorReferences,
};
use sideseat_ports::types::{
    FeedMessagesParams, FeedSpansParams, ListLogsParams, ListMetricsParams, ListSessionsParams,
    ListSpansParams, ListTracesParams, LogRow, MessageQueryParams, MessageQueryResult,
    MetricAggregateRow, MetricRow, NormalizedLog, NormalizedMetric, NormalizedSpan, ProjectId,
    RawPending, RawRecordRow, SearchPage, SearchQuery, SessionRow, SpanCounts, SpanRow, TraceRow,
    deduplicate_by_span_identity,
};

pub struct DedupAnalyticsRepository {
    inner: Box<dyn AnalyticsRepository + Send + Sync>,
}

impl DedupAnalyticsRepository {
    pub fn new(inner: Box<dyn AnalyticsRepository + Send + Sync>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl SpanStore for DedupAnalyticsRepository {
    // ==================== Span Operations (DEDUP Vec<SpanRow>) ====================

    async fn list_spans(&self, params: &ListSpansParams) -> Result<(Vec<SpanRow>, u64), DataError> {
        let (spans, total) = self.inner.list_spans(params).await?;
        Ok((deduplicate_by_span_identity(spans), total))
    }

    async fn get_spans_for_trace(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        limit: usize,
    ) -> Result<Vec<SpanRow>, DataError> {
        let spans = self
            .inner
            .get_spans_for_trace(project_id, trace_id, limit)
            .await?;
        Ok(deduplicate_by_span_identity(spans))
    }

    async fn get_span(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Option<SpanRow>, DataError> {
        self.inner.get_span(project_id, trace_id, span_id).await
    }

    async fn get_span_counts_bulk(
        &self,
        project_id: &ProjectId,
        span_keys: &[(String, String)],
    ) -> Result<HashMap<(String, String), SpanCounts>, DataError> {
        self.inner.get_span_counts_bulk(project_id, span_keys).await
    }

    async fn get_feed_spans(&self, params: &FeedSpansParams) -> Result<Vec<SpanRow>, DataError> {
        let spans = self.inner.get_feed_spans(params).await?;
        Ok(deduplicate_by_span_identity(spans))
    }

    async fn get_span_filter_options(
        &self,
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
        observations_only: bool,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        self.inner
            .get_span_filter_options(
                project_id,
                columns,
                from_timestamp,
                to_timestamp,
                observations_only,
            )
            .await
    }

    async fn delete_spans(
        &self,
        project_id: &ProjectId,
        span_keys: &[(String, String)],
    ) -> Result<u64, DataError> {
        self.inner.delete_spans(project_id, span_keys).await
    }

    // ==================== Ingestion Operations (pass-through) ====================

    async fn insert_spans(&self, spans: Vec<NormalizedSpan>) -> Result<(), DataError> {
        self.inner.insert_spans(spans).await
    }

    async fn spans_match_content(
        &self,
        project_id: &ProjectId,
        records: &[(String, String, String)],
    ) -> Result<bool, DataError> {
        self.inner.spans_match_content(project_id, records).await
    }

    async fn spans_with_matching_content(
        &self,
        project_id: &ProjectId,
        records: &[(String, String, String)],
    ) -> Result<HashSet<(String, String, String)>, DataError> {
        self.inner
            .spans_with_matching_content(project_id, records)
            .await
    }
}

#[async_trait]
impl MetricStore for DedupAnalyticsRepository {
    async fn insert_metrics(&self, metrics: &[NormalizedMetric]) -> Result<(), DataError> {
        self.inner.insert_metrics(metrics).await
    }

    async fn list_metrics(
        &self,
        params: &ListMetricsParams,
    ) -> Result<(Vec<MetricRow>, u64), DataError> {
        self.inner.list_metrics(params).await
    }

    async fn get_metric(
        &self,
        project_id: &ProjectId,
        datapoint_id: &str,
    ) -> Result<Option<MetricRow>, DataError> {
        self.inner.get_metric(project_id, datapoint_id).await
    }

    async fn aggregate_metrics(
        &self,
        params: &ListMetricsParams,
    ) -> Result<Vec<MetricAggregateRow>, DataError> {
        self.inner.aggregate_metrics(params).await
    }

    async fn get_metric_filter_options(
        &self,
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        self.inner
            .get_metric_filter_options(project_id, columns, from_timestamp, to_timestamp)
            .await
    }

    async fn metrics_match_content(
        &self,
        project_id: &ProjectId,
        records: &[(String, String)],
    ) -> Result<bool, DataError> {
        self.inner.metrics_match_content(project_id, records).await
    }
}

#[async_trait]
impl LogStore for DedupAnalyticsRepository {
    async fn insert_logs(&self, logs: &[NormalizedLog]) -> Result<(), DataError> {
        self.inner.insert_logs(logs).await
    }

    async fn list_logs(&self, params: &ListLogsParams) -> Result<(Vec<LogRow>, u64), DataError> {
        self.inner.list_logs(params).await
    }

    async fn get_log(
        &self,
        project_id: &ProjectId,
        log_digest: &str,
        ordinal: u32,
    ) -> Result<Option<LogRow>, DataError> {
        self.inner.get_log(project_id, log_digest, ordinal).await
    }

    async fn get_log_filter_options(
        &self,
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        self.inner
            .get_log_filter_options(project_id, columns, from_timestamp, to_timestamp)
            .await
    }

    async fn logs_match_content(
        &self,
        project_id: &ProjectId,
        records: &[(String, u32)],
    ) -> Result<bool, DataError> {
        self.inner.logs_match_content(project_id, records).await
    }
}

#[async_trait]
impl SearchIndex for DedupAnalyticsRepository {
    async fn search(&self, query: &SearchQuery) -> Result<SearchPage, DataError> {
        self.inner.search(query).await
    }

    async fn search_arrivals_detected(
        &self,
        query: &SearchQuery,
        through: &sideseat_ports::types::SearchCursor,
    ) -> Result<bool, DataError> {
        self.inner.search_arrivals_detected(query, through).await
    }

    async fn search_backfill_page(
        &self,
        project_id: &ProjectId,
        signal: sideseat_ports::types::SearchSignal,
        limit: usize,
    ) -> Result<Vec<sideseat_ports::types::SearchBackfillSource>, DataError> {
        self.inner
            .search_backfill_page(project_id, signal, limit)
            .await
    }

    async fn write_search_backfill(
        &self,
        project_id: &ProjectId,
        signal: sideseat_ports::types::SearchSignal,
        documents: &[sideseat_ports::types::SearchBackfillDocument],
    ) -> Result<(), DataError> {
        self.inner
            .write_search_backfill(project_id, signal, documents)
            .await
    }
}

#[async_trait]
impl EntityQuery for DedupAnalyticsRepository {
    // ==================== Trace Operations (pass-through) ====================

    async fn list_traces(
        &self,
        params: &ListTracesParams,
    ) -> Result<(Vec<TraceRow>, u64), DataError> {
        self.inner.list_traces(params).await
    }

    async fn get_trace(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
    ) -> Result<Option<TraceRow>, DataError> {
        self.inner.get_trace(project_id, trace_id).await
    }

    async fn get_trace_filter_options(
        &self,
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        self.inner
            .get_trace_filter_options(project_id, columns, from_timestamp, to_timestamp)
            .await
    }

    async fn get_trace_tags_options(
        &self,
        project_id: &ProjectId,
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<Vec<FilterOptionRow>, DataError> {
        self.inner
            .get_trace_tags_options(project_id, from_timestamp, to_timestamp)
            .await
    }

    async fn delete_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<u64, DataError> {
        self.inner.delete_traces(project_id, trace_ids).await
    }

    async fn traces_without_spans(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        self.inner.traces_without_spans(project_id, trace_ids).await
    }

    // ==================== Session Operations (pass-through) ====================

    async fn list_sessions(
        &self,
        params: &ListSessionsParams,
    ) -> Result<(Vec<SessionRow>, u64), DataError> {
        self.inner.list_sessions(params).await
    }

    async fn get_session(
        &self,
        project_id: &ProjectId,
        session_id: &str,
    ) -> Result<Option<SessionRow>, DataError> {
        self.inner.get_session(project_id, session_id).await
    }

    async fn get_traces_for_session(
        &self,
        project_id: &ProjectId,
        session_id: &str,
    ) -> Result<Vec<TraceRow>, DataError> {
        self.inner
            .get_traces_for_session(project_id, session_id)
            .await
    }

    async fn get_session_ids_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
        as_of_us: Option<i64>,
    ) -> Result<Vec<String>, DataError> {
        self.inner
            .get_session_ids_for_traces(project_id, trace_ids, as_of_us)
            .await
    }

    async fn get_trace_session_pairs(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
        as_of_us: Option<i64>,
    ) -> Result<Vec<(String, String)>, DataError> {
        self.inner
            .get_trace_session_pairs(project_id, trace_ids, as_of_us)
            .await
    }

    async fn get_trace_ids_for_sessions(
        &self,
        project_id: &ProjectId,
        session_ids: &[String],
        as_of_us: Option<i64>,
    ) -> Result<Vec<String>, DataError> {
        self.inner
            .get_trace_ids_for_sessions(project_id, session_ids, as_of_us)
            .await
    }

    async fn get_session_filter_options(
        &self,
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        self.inner
            .get_session_filter_options(project_id, columns, from_timestamp, to_timestamp)
            .await
    }

    async fn delete_sessions(
        &self,
        project_id: &ProjectId,
        session_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        self.inner.delete_sessions(project_id, session_ids).await
    }

    // ==================== Stats Operations (pass-through) ====================

    async fn get_project_stats(
        &self,
        params: &sideseat_ports::types::StatsParams,
    ) -> Result<sideseat_ports::types::ProjectStatsResult, DataError> {
        self.inner.get_project_stats(params).await
    }
}

#[async_trait]
impl MessageStore for DedupAnalyticsRepository {
    // ==================== Message Operations (DEDUP rows) ====================

    async fn get_messages(
        &self,
        params: &MessageQueryParams,
    ) -> Result<MessageQueryResult, DataError> {
        let mut result = self.inner.get_messages(params).await?;
        result.rows = deduplicate_by_span_identity(result.rows);
        Ok(result)
    }

    async fn get_project_messages(
        &self,
        params: &FeedMessagesParams,
    ) -> Result<MessageQueryResult, DataError> {
        let mut result = self.inner.get_project_messages(params).await?;
        result.rows = deduplicate_by_span_identity(result.rows);
        Ok(result)
    }
}

#[async_trait]
impl AnalyticsMaintenance for DedupAnalyticsRepository {
    // ==================== Project Data Operations (pass-through) ====================

    async fn analytics_project_ids(&self, limit: usize) -> Result<Vec<ProjectId>, DataError> {
        self.inner.analytics_project_ids(limit).await
    }

    async fn delete_project_data(&self, project_id: &ProjectId) -> Result<u64, DataError> {
        self.inner.delete_project_data(project_id).await
    }

    async fn count_project_rows(&self, project_id: &ProjectId) -> Result<u64, DataError> {
        // Deliberately not deduplicated: this asks whether *any* row is left, and a redelivered span that
        // dedup would hide is still a row that a deleted project must not own.
        self.inner.count_project_rows(project_id).await
    }

    async fn max_ingested_at_us(&self, project_id: &ProjectId) -> Result<Option<i64>, DataError> {
        // Not deduplicated either: this asks what the store has committed, and a re-delivery's newer row is
        // exactly the kind of commit a traversal watermark exists to account for.
        self.inner.max_ingested_at_us(project_id).await
    }

    async fn count_spans_by_project(
        &self,
        project_ids: &[ProjectId],
    ) -> Result<HashMap<String, u64>, DataError> {
        self.inner.count_spans_by_project(project_ids).await
    }

    async fn patch_project_hold(
        &self,
        project_id: &ProjectId,
        hold_until: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), DataError> {
        self.inner.patch_project_hold(project_id, hold_until).await
    }

    async fn project_logical_bytes(&self, project_id: &ProjectId) -> Result<u64, DataError> {
        self.inner.project_logical_bytes(project_id).await
    }

    async fn project_held_logical_bytes(
        &self,
        project_id: &ProjectId,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<u64, DataError> {
        self.inner.project_held_logical_bytes(project_id, now).await
    }

    async fn oldest_reclaimable_spans(
        &self,
        project_id: &ProjectId,
        target_bytes: u64,
        now: chrono::DateTime<chrono::Utc>,
        limit: usize,
    ) -> Result<Vec<sideseat_ports::types::PressureSpanCandidate>, DataError> {
        self.inner
            .oldest_reclaimable_spans(project_id, target_bytes, now, limit)
            .await
    }
}

#[async_trait]
impl SurvivorReferences for DedupAnalyticsRepository {
    async fn survivor_raw_records(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<Vec<u8>>, DataError> {
        self.inner.survivor_raw_records(project_id, trace_ids).await
    }

    async fn file_reference_fields_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        self.inner
            .file_reference_fields_for_traces(project_id, trace_ids)
            .await
    }
}

#[async_trait]
impl sideseat_ports::traits::RawStore for DedupAnalyticsRepository {
    async fn insert_raw_records(&self, records: &[RawRecordRow]) -> Result<(), DataError> {
        self.inner.insert_raw_records(records).await
    }

    async fn append_raw_records(&self, records: &[RawRecordRow]) -> Result<(), DataError> {
        self.inner.append_raw_records(records).await
    }

    async fn get_raw_records(
        &self,
        project_id: &ProjectId,
        raw_ids: &[String],
    ) -> Result<Vec<RawRecordRow>, DataError> {
        self.inner.get_raw_records(project_id, raw_ids).await
    }

    async fn raw_records_page(
        &self,
        project_id: &ProjectId,
        after: Option<(chrono::DateTime<chrono::Utc>, String)>,
        limit: usize,
    ) -> Result<Vec<RawRecordRow>, DataError> {
        self.inner.raw_records_page(project_id, after, limit).await
    }

    async fn delete_raw_records(
        &self,
        project_id: &ProjectId,
        raw_ids: &[String],
    ) -> Result<(), DataError> {
        self.inner.delete_raw_records(project_id, raw_ids).await
    }

    async fn span_raw_ids(
        &self,
        project_id: &ProjectId,
        spans: &[(String, String)],
    ) -> Result<HashMap<(String, String), String>, DataError> {
        self.inner.span_raw_ids(project_id, spans).await
    }

    async fn raw_records_named(
        &self,
        project_id: &ProjectId,
        raw_ids: &[String],
    ) -> Result<std::collections::HashSet<String>, DataError> {
        self.inner.raw_records_named(project_id, raw_ids).await
    }

    async fn enqueue_raw_records(
        &self,
        project_id: &ProjectId,
        raw_ids: &[String],
    ) -> Result<(), DataError> {
        self.inner.enqueue_raw_records(project_id, raw_ids).await
    }

    async fn pending_raw_records(&self, limit: usize) -> Result<Vec<RawPending>, DataError> {
        self.inner.pending_raw_records(limit).await
    }

    async fn clear_raw_pending(&self, entries: &[RawPending]) -> Result<(), DataError> {
        self.inner.clear_raw_pending(entries).await
    }
}
