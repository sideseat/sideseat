//! AnalyticsRepository trait implementation for DuckDB
//!
//! This module implements the AnalyticsRepository trait for Arc<DuckdbService>,
//! wrapping the synchronous DuckDB operations in async wrappers.
//!
//! Note: The trait is implemented for Arc<DuckdbService> rather than DuckdbService
//! directly because the mutex guard protecting the DuckDB connection is not Send,
//! so we need to clone the Arc and get the connection inside the spawn_blocking closure.

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

use super::DuckdbService;
use super::repositories::{messages, metric, query, span, stats};

#[async_trait]
impl AnalyticsRepository for Arc<DuckdbService> {
    // ==================== Trace Operations ====================

    async fn list_traces(
        &self,
        params: &ListTracesParams,
    ) -> Result<(Vec<TraceRow>, u64), DataError> {
        let db = Arc::clone(self);
        let params = params.clone();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::list_traces(&conn, &params)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn get_trace(
        &self,
        project_id: &str,
        trace_id: &str,
    ) -> Result<Option<TraceRow>, DataError> {
        let db = Arc::clone(self);
        let pid = project_id.to_string();
        let tid = trace_id.to_string();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::get_trace(&conn, &pid, &tid)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn get_trace_filter_options(
        &self,
        project_id: &str,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        let db = Arc::clone(self);
        let pid = project_id.to_string();
        let cols = columns.to_vec();
        let result = DuckdbService::run_query(move || {
            let conn = db.conn();
            query::get_trace_filter_options(&conn, &pid, &cols, from_timestamp, to_timestamp)
        })
        .await
        .map_err(DataError::from)?
        .map_err(DataError::from)?;

        // Convert from query::FilterOptionRow to traits::FilterOptionRow
        Ok(result
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    v.into_iter()
                        .map(|r| FilterOptionRow {
                            value: r.value,
                            count: r.count,
                        })
                        .collect(),
                )
            })
            .collect())
    }

    async fn get_trace_tags_options(
        &self,
        project_id: &str,
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<Vec<FilterOptionRow>, DataError> {
        let db = Arc::clone(self);
        let pid = project_id.to_string();
        let result = DuckdbService::run_query(move || {
            let conn = db.conn();
            query::get_trace_tags_options(&conn, &pid, from_timestamp, to_timestamp)
        })
        .await
        .map_err(DataError::from)?
        .map_err(DataError::from)?;

        Ok(result
            .into_iter()
            .map(|r| FilterOptionRow {
                value: r.value,
                count: r.count,
            })
            .collect())
    }

    async fn delete_traces(
        &self,
        project_id: &str,
        trace_ids: &[String],
    ) -> Result<u64, DataError> {
        let db = Arc::clone(self);
        let pid = project_id.to_string();
        let tids = trace_ids.to_vec();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::delete_traces(&conn, &pid, &tids)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    // ==================== Span Operations ====================

    async fn list_spans(&self, params: &ListSpansParams) -> Result<(Vec<SpanRow>, u64), DataError> {
        let db = Arc::clone(self);
        let params = params.clone();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::list_spans(&conn, &params)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn get_spans_for_trace(
        &self,
        project_id: &str,
        trace_id: &str,
    ) -> Result<Vec<SpanRow>, DataError> {
        let db = Arc::clone(self);
        let pid = project_id.to_string();
        let tid = trace_id.to_string();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::get_spans_for_trace(&conn, &pid, &tid)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn get_span(
        &self,
        project_id: &str,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Option<SpanRow>, DataError> {
        let db = Arc::clone(self);
        let pid = project_id.to_string();
        let tid = trace_id.to_string();
        let sid = span_id.to_string();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::get_span(&conn, &pid, &tid, &sid)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn get_events_for_span(
        &self,
        project_id: &str,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Vec<EventRow>, DataError> {
        let db = Arc::clone(self);
        let pid = project_id.to_string();
        let tid = trace_id.to_string();
        let sid = span_id.to_string();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::get_events_for_span(&conn, &pid, &tid, &sid)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn get_links_for_span(
        &self,
        project_id: &str,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Vec<LinkRow>, DataError> {
        let db = Arc::clone(self);
        let pid = project_id.to_string();
        let tid = trace_id.to_string();
        let sid = span_id.to_string();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::get_links_for_span(&conn, &pid, &tid, &sid)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn get_span_counts_bulk(
        &self,
        project_id: &str,
        span_keys: &[(String, String)],
    ) -> Result<HashMap<(String, String), SpanCounts>, DataError> {
        let db = Arc::clone(self);
        let pid = project_id.to_string();
        let keys = span_keys.to_vec();
        let result = DuckdbService::run_query(move || {
            let conn = db.conn();
            query::get_span_counts_bulk(&conn, &pid, &keys)
        })
        .await
        .map_err(DataError::from)?
        .map_err(DataError::from)?;

        // Convert from query::SpanCounts to types::SpanCounts
        Ok(result
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    SpanCounts {
                        event_count: v.event_count,
                        link_count: v.link_count,
                    },
                )
            })
            .collect())
    }

    async fn get_feed_spans(&self, params: &FeedSpansParams) -> Result<Vec<SpanRow>, DataError> {
        let db = Arc::clone(self);
        let params = params.clone();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::get_feed_spans(&conn, &params)
        })
        .await
        .map_err(DataError::from)?
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
        let db = Arc::clone(self);
        let pid = project_id.to_string();
        let cols = columns.to_vec();
        let result = DuckdbService::run_query(move || {
            let conn = db.conn();
            query::get_span_filter_options(
                &conn,
                &pid,
                &cols,
                from_timestamp,
                to_timestamp,
                observations_only,
            )
        })
        .await
        .map_err(DataError::from)?
        .map_err(DataError::from)?;

        Ok(result
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    v.into_iter()
                        .map(|r| FilterOptionRow {
                            value: r.value,
                            count: r.count,
                        })
                        .collect(),
                )
            })
            .collect())
    }

    async fn delete_spans(
        &self,
        project_id: &str,
        span_keys: &[(String, String)],
    ) -> Result<u64, DataError> {
        let db = Arc::clone(self);
        let pid = project_id.to_string();
        let keys = span_keys.to_vec();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::delete_spans(&conn, &pid, &keys)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    // ==================== Session Operations ====================

    async fn list_sessions(
        &self,
        params: &ListSessionsParams,
    ) -> Result<(Vec<SessionRow>, u64), DataError> {
        let db = Arc::clone(self);
        let params = params.clone();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::list_sessions(&conn, &params)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn get_session(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<Option<SessionRow>, DataError> {
        let db = Arc::clone(self);
        let pid = project_id.to_string();
        let sid = session_id.to_string();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::get_session(&conn, &pid, &sid)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn get_traces_for_session(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<Vec<TraceRow>, DataError> {
        let db = Arc::clone(self);
        let pid = project_id.to_string();
        let sid = session_id.to_string();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::get_traces_for_session(&conn, &pid, &sid)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn get_trace_ids_for_sessions(
        &self,
        project_id: &str,
        session_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        let db = Arc::clone(self);
        let pid = project_id.to_string();
        let sids = session_ids.to_vec();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::get_trace_ids_for_sessions(&conn, &pid, &sids)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn get_session_filter_options(
        &self,
        project_id: &str,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        let db = Arc::clone(self);
        let pid = project_id.to_string();
        let cols = columns.to_vec();
        let result = DuckdbService::run_query(move || {
            let conn = db.conn();
            query::get_session_filter_options(&conn, &pid, &cols, from_timestamp, to_timestamp)
        })
        .await
        .map_err(DataError::from)?
        .map_err(DataError::from)?;

        Ok(result
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    v.into_iter()
                        .map(|r| FilterOptionRow {
                            value: r.value,
                            count: r.count,
                        })
                        .collect(),
                )
            })
            .collect())
    }

    async fn delete_sessions(
        &self,
        project_id: &str,
        session_ids: &[String],
    ) -> Result<u64, DataError> {
        let db = Arc::clone(self);
        let pid = project_id.to_string();
        let sids = session_ids.to_vec();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::delete_sessions(&conn, &pid, &sids)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    // ==================== Message Operations ====================

    async fn get_messages(
        &self,
        params: &MessageQueryParams,
    ) -> Result<MessageQueryResult, DataError> {
        let db = Arc::clone(self);
        let params = params.clone();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            messages::get_messages(&conn, &params)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn get_project_messages(
        &self,
        params: &FeedMessagesParams,
    ) -> Result<MessageQueryResult, DataError> {
        let db = Arc::clone(self);
        let params = params.clone();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            messages::get_project_messages(&conn, &params)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    // ==================== Stats Operations ====================

    async fn get_project_stats(
        &self,
        params: &StatsParams,
    ) -> Result<ProjectStatsResult, DataError> {
        let db = Arc::clone(self);
        let params = params.clone();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            stats::get_project_stats(&conn, &params)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    // ==================== Ingestion Operations ====================

    async fn insert_spans(&self, spans: Vec<NormalizedSpan>) -> Result<(), DataError> {
        let db = Arc::clone(self);
        DuckdbService::run_query(move || {
            let conn = db.conn();
            span::insert_batch(&conn, &spans)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn insert_metrics(&self, metrics: &[NormalizedMetric]) -> Result<(), DataError> {
        let db = Arc::clone(self);
        let metrics = metrics.to_vec();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            metric::insert_batch(&conn, &metrics)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    // ==================== Project Data Operations ====================

    async fn delete_project_data(&self, project_id: &str) -> Result<u64, DataError> {
        let db = Arc::clone(self);
        let pid = project_id.to_string();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::delete_project_data(&conn, &pid)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }
}
