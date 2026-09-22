//! Analytics repository trait implementation for DuckDB.
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

use sideseat_ports::error::DataError;
use sideseat_ports::traits::{
    AnalyticsMaintenance, EntityQuery, FilterOptionRow, LogStore, MessageStore, MetricStore,
    SearchIndex, SpanStore, SurvivorReferences,
};
use sideseat_ports::types::{
    EventRow, FeedMessagesParams, FeedSpansParams, LinkRow, ListLogsParams, ListMetricsParams,
    ListSessionsParams, ListSpansParams, ListTracesParams, LogRow, MessageQueryParams,
    MessageQueryResult, MetricAggregateRow, MetricRow, NormalizedLog, NormalizedMetric,
    NormalizedSpan, PressureSpanCandidate, ProjectId, ProjectStatsResult, SearchPage, SearchQuery,
    SessionRow, SpanCounts, SpanRow, StatsParams, TraceRow,
};

use super::DuckdbService;
use super::repositories::{log, messages, metric, query, search, span, stats};

/// The port, implemented over the service.
///
/// A **wrapper rather than `impl … for Arc<DuckdbService>`**, and that is the orphan rule rather than taste: with the
/// trait in `sideseat-ports` and `Arc` in `std`, an impl on `Arc<DuckdbService>` has no local type ahead of an
/// uncovered parameter, so it is refused across a crate boundary. It compiled only while everything was one
/// crate - which is one more way the single crate hid the direction of its own dependencies.
#[derive(Clone)]
pub struct DuckdbRepository(pub Arc<DuckdbService>);

/// So the wrapper is transparent to the service's own methods.
///
/// Without this, wrapping turns every call that is *not* a port method - a maintenance helper, a test probe -
/// into `wrapper.0.method()`, which is noise that says nothing. The port methods live on the wrapper itself and
/// are found first, so nothing is shadowed.
impl std::ops::Deref for DuckdbRepository {
    type Target = DuckdbService;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[async_trait]
impl SpanStore for DuckdbRepository {
    // ==================== Span Operations ====================

    async fn list_spans(&self, params: &ListSpansParams) -> Result<(Vec<SpanRow>, u64), DataError> {
        let db = Arc::clone(&self.0);
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
        project_id: &ProjectId,
        trace_id: &str,
    ) -> Result<Vec<SpanRow>, DataError> {
        let db = Arc::clone(&self.0);
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
        project_id: &ProjectId,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Option<SpanRow>, DataError> {
        let db = Arc::clone(&self.0);
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
        project_id: &ProjectId,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Vec<EventRow>, DataError> {
        let db = Arc::clone(&self.0);
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
        project_id: &ProjectId,
        trace_id: &str,
        span_id: &str,
    ) -> Result<Vec<LinkRow>, DataError> {
        let db = Arc::clone(&self.0);
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
        project_id: &ProjectId,
        span_keys: &[(String, String)],
    ) -> Result<HashMap<(String, String), SpanCounts>, DataError> {
        let db = Arc::clone(&self.0);
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
        let db = Arc::clone(&self.0);
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
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
        observations_only: bool,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        let db = Arc::clone(&self.0);
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
        project_id: &ProjectId,
        span_keys: &[(String, String)],
    ) -> Result<u64, DataError> {
        let db = Arc::clone(&self.0);
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

    // ==================== Ingestion Operations ====================

    async fn insert_spans(&self, mut spans: Vec<NormalizedSpan>) -> Result<(), DataError> {
        let now = self.0.clock().now();
        for span in &mut spans {
            span.ingested_at.get_or_insert(now);
        }
        let db = Arc::clone(&self.0);
        DuckdbService::run_query(move || {
            let conn = db.conn();
            span::insert_batch(&conn, &spans)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn spans_match_content(
        &self,
        project_id: &ProjectId,
        records: &[(String, String, String)],
    ) -> Result<bool, DataError> {
        let db = Arc::clone(&self.0);
        let project_id = project_id.to_string();
        let records = records.to_vec();
        DuckdbService::run_query(move || {
            query::spans_match_content(&db.conn(), &project_id, &records)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }
}

#[async_trait]
impl MetricStore for DuckdbRepository {
    async fn insert_metrics(&self, metrics: &[NormalizedMetric]) -> Result<(), DataError> {
        let db = Arc::clone(&self.0);
        let metrics = metrics.to_vec();
        let now = self.0.clock().now();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            metric::insert_batch(&conn, &metrics, now)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn list_metrics(
        &self,
        params: &ListMetricsParams,
    ) -> Result<(Vec<MetricRow>, u64), DataError> {
        let db = Arc::clone(&self.0);
        let params = params.clone();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            metric::list_metrics(&conn, &params)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn get_metric(
        &self,
        project_id: &ProjectId,
        datapoint_id: &str,
    ) -> Result<Option<MetricRow>, DataError> {
        let db = Arc::clone(&self.0);
        let project_id = project_id.clone();
        let datapoint_id = datapoint_id.to_string();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            metric::get_metric(&conn, &project_id, &datapoint_id)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn aggregate_metrics(
        &self,
        params: &ListMetricsParams,
    ) -> Result<Vec<MetricAggregateRow>, DataError> {
        let db = Arc::clone(&self.0);
        let params = params.clone();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            metric::aggregate_metrics(&conn, &params)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn get_metric_filter_options(
        &self,
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        let db = Arc::clone(&self.0);
        let project_id = project_id.clone();
        let columns = columns.to_vec();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            metric::get_metric_filter_options(
                &conn,
                &project_id,
                &columns,
                from_timestamp,
                to_timestamp,
            )
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn metrics_match_content(
        &self,
        project_id: &ProjectId,
        records: &[(String, String)],
    ) -> Result<bool, DataError> {
        let db = Arc::clone(&self.0);
        let project_id = project_id.clone();
        let records = records.to_vec();
        DuckdbService::run_query(move || metric::matches_content(&db.conn(), &project_id, &records))
            .await
            .map_err(DataError::from)?
            .map_err(Into::into)
    }
}

#[async_trait]
impl LogStore for DuckdbRepository {
    async fn insert_logs(&self, logs: &[NormalizedLog]) -> Result<(), DataError> {
        let db = Arc::clone(&self.0);
        let mut logs = logs.to_vec();
        let now = self.0.clock().now();
        for log in &mut logs {
            log.ingested_at.get_or_insert(now);
        }
        DuckdbService::run_query(move || {
            let conn = db.conn();
            log::insert_batch(&conn, &logs)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn list_logs(&self, params: &ListLogsParams) -> Result<(Vec<LogRow>, u64), DataError> {
        let db = Arc::clone(&self.0);
        let params = params.clone();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            log::list_logs(&conn, &params)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn get_log(
        &self,
        project_id: &ProjectId,
        log_digest: &str,
        ordinal: u32,
    ) -> Result<Option<LogRow>, DataError> {
        let db = Arc::clone(&self.0);
        let project_id = project_id.clone();
        let log_digest = log_digest.to_string();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            log::get_log(&conn, &project_id, &log_digest, ordinal)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn get_log_filter_options(
        &self,
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        let db = Arc::clone(&self.0);
        let project_id = project_id.clone();
        let columns = columns.to_vec();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            log::get_log_filter_options(&conn, &project_id, &columns, from_timestamp, to_timestamp)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn logs_match_content(
        &self,
        project_id: &ProjectId,
        records: &[(String, u32)],
    ) -> Result<bool, DataError> {
        let db = Arc::clone(&self.0);
        let project_id = project_id.clone();
        let records = records.to_vec();
        DuckdbService::run_query(move || log::matches_content(&db.conn(), &project_id, &records))
            .await
            .map_err(DataError::from)?
            .map_err(Into::into)
    }
}

#[async_trait]
impl SearchIndex for DuckdbRepository {
    async fn search(&self, request: &SearchQuery) -> Result<SearchPage, DataError> {
        let db = Arc::clone(&self.0);
        let request = request.clone();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            search::search(&conn, &request)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }
}

#[async_trait]
impl EntityQuery for DuckdbRepository {
    // ==================== Trace Operations ====================

    async fn list_traces(
        &self,
        params: &ListTracesParams,
    ) -> Result<(Vec<TraceRow>, u64), DataError> {
        let db = Arc::clone(&self.0);
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
        project_id: &ProjectId,
        trace_id: &str,
    ) -> Result<Option<TraceRow>, DataError> {
        let db = Arc::clone(&self.0);
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
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        let db = Arc::clone(&self.0);
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
        project_id: &ProjectId,
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<Vec<FilterOptionRow>, DataError> {
        let db = Arc::clone(&self.0);
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

    async fn traces_without_spans(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        let db = Arc::clone(&self.0);
        let pid = project_id.to_string();
        let tids = trace_ids.to_vec();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::traces_without_spans(&conn, &pid, &tids)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn delete_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<u64, DataError> {
        let db = Arc::clone(&self.0);
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

    // ==================== Session Operations ====================

    async fn list_sessions(
        &self,
        params: &ListSessionsParams,
    ) -> Result<(Vec<SessionRow>, u64), DataError> {
        let db = Arc::clone(&self.0);
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
        project_id: &ProjectId,
        session_id: &str,
    ) -> Result<Option<SessionRow>, DataError> {
        let db = Arc::clone(&self.0);
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
        project_id: &ProjectId,
        session_id: &str,
    ) -> Result<Vec<TraceRow>, DataError> {
        let db = Arc::clone(&self.0);
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

    async fn get_trace_session_pairs(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
        as_of_us: Option<i64>,
    ) -> Result<Vec<(String, String)>, DataError> {
        let db = Arc::clone(&self.0);
        let pid = project_id.to_string();
        let tids = trace_ids.to_vec();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::get_trace_session_pairs(&conn, &pid, &tids, as_of_us)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn get_session_ids_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
        as_of_us: Option<i64>,
    ) -> Result<Vec<String>, DataError> {
        let db = Arc::clone(&self.0);
        let pid = project_id.to_string();
        let tids = trace_ids.to_vec();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::get_session_ids_for_traces(&conn, &pid, &tids, as_of_us)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn get_trace_ids_for_sessions(
        &self,
        project_id: &ProjectId,
        session_ids: &[String],
        as_of_us: Option<i64>,
    ) -> Result<Vec<String>, DataError> {
        let db = Arc::clone(&self.0);
        let pid = project_id.to_string();
        let sids = session_ids.to_vec();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::get_trace_ids_for_sessions(&conn, &pid, &sids, as_of_us)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn get_session_filter_options(
        &self,
        project_id: &ProjectId,
        columns: &[String],
        from_timestamp: Option<DateTime<Utc>>,
        to_timestamp: Option<DateTime<Utc>>,
    ) -> Result<HashMap<String, Vec<FilterOptionRow>>, DataError> {
        let db = Arc::clone(&self.0);
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
        project_id: &ProjectId,
        session_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        let db = Arc::clone(&self.0);
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

    // ==================== Stats Operations ====================

    async fn get_project_stats(
        &self,
        params: &StatsParams,
    ) -> Result<ProjectStatsResult, DataError> {
        let db = Arc::clone(&self.0);
        let params = params.clone();
        let now = self.0.clock().now();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            stats::get_project_stats(&conn, &params, now)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }
}

#[async_trait]
impl MessageStore for DuckdbRepository {
    // ==================== Message Operations ====================

    async fn get_messages(
        &self,
        params: &MessageQueryParams,
    ) -> Result<MessageQueryResult, DataError> {
        let db = Arc::clone(&self.0);
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
        let db = Arc::clone(&self.0);
        let params = params.clone();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            messages::get_project_messages(&conn, &params)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }
}

#[async_trait]
impl AnalyticsMaintenance for DuckdbRepository {
    // ==================== Project Data Operations ====================

    async fn delete_project_data(&self, project_id: &ProjectId) -> Result<u64, DataError> {
        let db = Arc::clone(&self.0);
        let pid = project_id.to_string();
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::delete_project_data(&conn, &pid)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn count_project_rows(&self, project_id: &ProjectId) -> Result<u64, DataError> {
        let db = Arc::clone(&self.0);
        let id = project_id.to_string();
        DuckdbService::run_query(move || {
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
        DuckdbService::run_query(move || {
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
        DuckdbService::run_query(move || {
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
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::patch_project_hold(&conn, &id, hold_until)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }

    async fn project_logical_bytes(&self, project_id: &ProjectId) -> Result<u64, DataError> {
        let db = Arc::clone(&self.0);
        let id = project_id.to_string();
        DuckdbService::run_query(move || {
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
        DuckdbService::run_query(move || {
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
        DuckdbService::run_query(move || {
            let conn = db.conn();
            query::oldest_reclaimable_spans(&conn, &id, target_bytes, now, limit)
        })
        .await
        .map_err(DataError::from)?
        .map_err(Into::into)
    }
}

#[async_trait]
impl SurvivorReferences for DuckdbRepository {
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
