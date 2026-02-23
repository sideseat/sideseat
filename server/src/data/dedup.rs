//! Deduplication wrapper for AnalyticsRepository
//!
//! Wraps any AnalyticsRepository and deduplicates SpanRow/MessageSpanRow
//! results in Rust. Aggregation queries use SQL-level dedup directly.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

use crate::data::error::DataError;
use crate::data::traits::{AnalyticsRepository, FilterOptionRow};
use crate::data::types::{
    EventRow, FeedMessagesParams, FeedSpansParams, LinkRow, ListSessionsParams, ListSpansParams,
    ListTracesParams, MessageQueryParams, MessageQueryResult, NormalizedMetric, NormalizedSpan,
    SessionRow, SpanCounts, SpanRow, TraceRow, deduplicate_by_span_identity,
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
impl AnalyticsRepository for DedupAnalyticsRepository {
    // ==================== Trace Operations (pass-through) ====================

    async fn list_traces(
        &self,
        params: &ListTracesParams,
    ) -> Result<(Vec<TraceRow>, u64), DataError> {
        self.inner.list_traces(params).await
    }

    async fn get_trace(
        &self,
        project_id: &str,
        trace_id: &str,
    ) -> Result<Option<TraceRow>, DataError> {
        self.inner.get_trace(project_id, trace_id).await
    }

    async fn get_trace_filter_options(
        &self,
        project_id: &str,
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
        project_id: &str,
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<Vec<FilterOptionRow>, DataError> {
        self.inner
            .get_trace_tags_options(project_id, from_timestamp, to_timestamp)
            .await
    }

    async fn delete_traces(
        &self,
        project_id: &str,
        trace_ids: &[String],
    ) -> Result<u64, DataError> {
        self.inner.delete_traces(project_id, trace_ids).await
    }

    // ==================== Span Operations (DEDUP Vec<SpanRow>) ====================

    async fn list_spans(&self, params: &ListSpansParams) -> Result<(Vec<SpanRow>, u64), DataError> {
        let (spans, total) = self.inner.list_spans(params).await?;
        Ok((deduplicate_by_span_identity(spans), total))
    }

    async fn get_spans_for_trace(
        &self,
        project_id: &str,
        trace_id: &str,
    ) -> Result<Vec<SpanRow>, DataError> {
        let spans = self.inner.get_spans_for_trace(project_id, trace_id).await?;
        Ok(deduplicate_by_span_identity(spans))
    }

    async fn get_span(
        &self,
        project_id: &str,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Option<SpanRow>, DataError> {
        self.inner.get_span(project_id, trace_id, span_id).await
    }

    async fn get_events_for_span(
        &self,
        project_id: &str,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Vec<EventRow>, DataError> {
        self.inner
            .get_events_for_span(project_id, trace_id, span_id)
            .await
    }

    async fn get_links_for_span(
        &self,
        project_id: &str,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Vec<LinkRow>, DataError> {
        self.inner
            .get_links_for_span(project_id, trace_id, span_id)
            .await
    }

    async fn get_span_counts_bulk(
        &self,
        project_id: &str,
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
        project_id: &str,
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
        project_id: &str,
        span_keys: &[(String, String)],
    ) -> Result<u64, DataError> {
        self.inner.delete_spans(project_id, span_keys).await
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
        project_id: &str,
        session_id: &str,
    ) -> Result<Option<SessionRow>, DataError> {
        self.inner.get_session(project_id, session_id).await
    }

    async fn get_traces_for_session(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<Vec<TraceRow>, DataError> {
        self.inner
            .get_traces_for_session(project_id, session_id)
            .await
    }

    async fn get_trace_ids_for_sessions(
        &self,
        project_id: &str,
        session_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        self.inner
            .get_trace_ids_for_sessions(project_id, session_ids)
            .await
    }

    async fn get_session_filter_options(
        &self,
        project_id: &str,
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
        project_id: &str,
        session_ids: &[String],
    ) -> Result<u64, DataError> {
        self.inner.delete_sessions(project_id, session_ids).await
    }

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

    // ==================== Stats Operations (pass-through) ====================

    async fn get_project_stats(
        &self,
        params: &crate::data::types::StatsParams,
    ) -> Result<crate::data::types::ProjectStatsResult, DataError> {
        self.inner.get_project_stats(params).await
    }

    // ==================== Ingestion Operations (pass-through) ====================

    async fn insert_spans(&self, spans: Vec<NormalizedSpan>) -> Result<(), DataError> {
        self.inner.insert_spans(spans).await
    }

    async fn insert_metrics(&self, metrics: &[NormalizedMetric]) -> Result<(), DataError> {
        self.inner.insert_metrics(metrics).await
    }

    // ==================== Project Data Operations (pass-through) ====================

    async fn delete_project_data(&self, project_id: &str) -> Result<u64, DataError> {
        self.inner.delete_project_data(project_id).await
    }

    async fn count_spans_by_project(
        &self,
        project_ids: &[String],
    ) -> Result<HashMap<String, u64>, DataError> {
        self.inner.count_spans_by_project(project_ids).await
    }
}
