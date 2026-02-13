//! AnalyticsRepository trait implementation for ClickHouse
//!
//! This module implements the AnalyticsRepository trait for Arc<ClickhouseService>.
//! ClickHouse operations are natively async so no spawn_blocking needed.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::data::error::DataError;
use crate::data::traits::{AnalyticsRepository, FilterOptionRow};
use crate::data::types::{
    EventRow, FeedMessagesParams, FeedSpansParams, LinkRow, ListSessionsParams, ListSpansParams,
    ListTracesParams, MessageQueryParams, MessageQueryResult, NormalizedMetric, NormalizedSpan,
    ProjectStatsResult, SessionRow, SpanCounts, SpanRow, StatsParams, TraceRow,
};

use super::ClickhouseService;
use super::repositories::{messages, metric, query, span, stats};

#[async_trait]
impl AnalyticsRepository for Arc<ClickhouseService> {
    // ==================== Trace Operations ====================

    async fn list_traces(
        &self,
        params: &ListTracesParams,
    ) -> Result<(Vec<TraceRow>, u64), DataError> {
        query::list_traces(self.client(), params)
            .await
            .map_err(Into::into)
    }

    async fn get_trace(
        &self,
        project_id: &str,
        trace_id: &str,
    ) -> Result<Option<TraceRow>, DataError> {
        query::get_trace(self.client(), project_id, trace_id)
            .await
            .map_err(Into::into)
    }

    async fn get_trace_filter_options(
        &self,
        project_id: &str,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        query::get_trace_filter_options(
            self.client(),
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
        project_id: &str,
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<Vec<FilterOptionRow>, DataError> {
        query::get_trace_tags_options(self.client(), project_id, from_timestamp, to_timestamp)
            .await
            .map_err(Into::into)
    }

    async fn delete_traces(
        &self,
        project_id: &str,
        trace_ids: &[String],
    ) -> Result<u64, DataError> {
        let table = self.delete_table("otel_spans");
        let on_cluster = self.on_cluster_clause();
        query::delete_traces(self.client(), &table, &on_cluster, project_id, trace_ids)
            .await
            .map_err(Into::into)
    }

    // ==================== Span Operations ====================

    async fn list_spans(&self, params: &ListSpansParams) -> Result<(Vec<SpanRow>, u64), DataError> {
        query::list_spans(self.client(), params)
            .await
            .map_err(Into::into)
    }

    async fn get_spans_for_trace(
        &self,
        project_id: &str,
        trace_id: &str,
    ) -> Result<Vec<SpanRow>, DataError> {
        query::get_spans_for_trace(self.client(), project_id, trace_id)
            .await
            .map_err(Into::into)
    }

    async fn get_span(
        &self,
        project_id: &str,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Option<SpanRow>, DataError> {
        query::get_span(self.client(), project_id, trace_id, span_id)
            .await
            .map_err(Into::into)
    }

    async fn get_events_for_span(
        &self,
        project_id: &str,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Vec<EventRow>, DataError> {
        query::get_events_for_span(self.client(), project_id, trace_id, span_id)
            .await
            .map_err(Into::into)
    }

    async fn get_links_for_span(
        &self,
        project_id: &str,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Vec<LinkRow>, DataError> {
        query::get_links_for_span(self.client(), project_id, trace_id, span_id)
            .await
            .map_err(Into::into)
    }

    async fn get_span_counts_bulk(
        &self,
        project_id: &str,
        span_keys: &[(String, String)],
    ) -> Result<HashMap<(String, String), SpanCounts>, DataError> {
        query::get_span_counts_bulk(self.client(), project_id, span_keys)
            .await
            .map_err(Into::into)
    }

    async fn get_feed_spans(&self, params: &FeedSpansParams) -> Result<Vec<SpanRow>, DataError> {
        query::get_feed_spans(self.client(), params)
            .await
            .map_err(Into::into)
    }

    async fn get_span_filter_options(
        &self,
        project_id: &str,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
        observations_only: bool,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        query::get_span_filter_options(
            self.client(),
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
        project_id: &str,
        span_keys: &[(String, String)],
    ) -> Result<u64, DataError> {
        let table = self.delete_table("otel_spans");
        let on_cluster = self.on_cluster_clause();
        query::delete_spans(self.client(), &table, &on_cluster, project_id, span_keys)
            .await
            .map_err(Into::into)
    }

    // ==================== Session Operations ====================

    async fn list_sessions(
        &self,
        params: &ListSessionsParams,
    ) -> Result<(Vec<SessionRow>, u64), DataError> {
        query::list_sessions(self.client(), params)
            .await
            .map_err(Into::into)
    }

    async fn get_session(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<Option<SessionRow>, DataError> {
        query::get_session(self.client(), project_id, session_id)
            .await
            .map_err(Into::into)
    }

    async fn get_traces_for_session(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<Vec<TraceRow>, DataError> {
        query::get_traces_for_session(self.client(), project_id, session_id)
            .await
            .map_err(Into::into)
    }

    async fn get_trace_ids_for_sessions(
        &self,
        project_id: &str,
        session_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        query::get_trace_ids_for_sessions(self.client(), project_id, session_ids)
            .await
            .map_err(Into::into)
    }

    async fn get_session_filter_options(
        &self,
        project_id: &str,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        query::get_session_filter_options(
            self.client(),
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
        project_id: &str,
        session_ids: &[String],
    ) -> Result<u64, DataError> {
        let table = self.delete_table("otel_spans");
        let on_cluster = self.on_cluster_clause();
        query::delete_sessions(self.client(), &table, &on_cluster, project_id, session_ids)
            .await
            .map_err(Into::into)
    }

    // ==================== Message Operations ====================

    async fn get_messages(
        &self,
        params: &MessageQueryParams,
    ) -> Result<MessageQueryResult, DataError> {
        messages::get_messages(self.client(), params)
            .await
            .map_err(Into::into)
    }

    async fn get_project_messages(
        &self,
        params: &FeedMessagesParams,
    ) -> Result<MessageQueryResult, DataError> {
        messages::get_project_messages(self.client(), params)
            .await
            .map_err(Into::into)
    }

    // ==================== Stats Operations ====================

    async fn get_project_stats(
        &self,
        params: &StatsParams,
    ) -> Result<ProjectStatsResult, DataError> {
        stats::get_project_stats(self.client(), params)
            .await
            .map_err(Into::into)
    }

    // ==================== Ingestion Operations ====================

    async fn insert_spans(&self, spans: &[NormalizedSpan]) -> Result<(), DataError> {
        // Use local table for distributed mode for optimal insert performance
        let table = self.insert_table("otel_spans");
        span::insert_batch(self.client(), &table, spans)
            .await
            .map_err(Into::into)
    }

    async fn insert_metrics(&self, metrics: &[NormalizedMetric]) -> Result<(), DataError> {
        // Use local table for distributed mode for optimal insert performance
        let table = self.insert_table("otel_metrics");
        metric::insert_batch(self.client(), &table, metrics)
            .await
            .map_err(Into::into)
    }

    // ==================== Project Data Operations ====================

    async fn delete_project_data(&self, project_id: &str) -> Result<u64, DataError> {
        let spans_table = self.delete_table("otel_spans");
        let metrics_table = self.delete_table("otel_metrics");
        let on_cluster = self.on_cluster_clause();
        query::delete_project_data(
            self.client(),
            &spans_table,
            &metrics_table,
            &on_cluster,
            project_id,
        )
        .await
        .map_err(Into::into)
    }
}
