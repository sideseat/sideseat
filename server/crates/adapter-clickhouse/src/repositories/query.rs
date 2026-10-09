//! Query repository for OTEL API queries (ClickHouse backend).
//!
//! Provides the same interface as DuckDB query repository but uses ClickHouse SQL.

mod rows;

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use clickhouse::{Client, Row};
use serde::Deserialize;

use sideseat_query_sql::{Backend, analytics, confirmations, dml};

pub(super) fn bind_analytics_values(
    mut query: clickhouse::query::Query,
    values: &[analytics::QueryValue],
) -> clickhouse::query::Query {
    for value in values {
        query = match value {
            analytics::QueryValue::String(value) => query.bind(value),
            analytics::QueryValue::Int64(value) => query.bind(value),
            analytics::QueryValue::Float64(value) => query.bind(value),
        };
    }
    query
}

pub async fn spans_match_content(
    client: &Client,
    project_id: &str,
    records: &[(String, String, String)],
) -> Result<bool, ClickhouseError> {
    let Some(plan) = confirmations::spans(project_id, records, Backend::Clickhouse) else {
        return Ok(true);
    };
    let found: u64 = bind_analytics_values(client.query(plan.query.sql()), plan.query.params())
        .fetch_one()
        .await?;
    Ok(found == plan.expected)
}

pub async fn spans_with_matching_content(
    client: &Client,
    project_id: &str,
    records: &[(String, String, String)],
) -> Result<HashSet<(String, String, String)>, ClickhouseError> {
    let Some(query) = confirmations::matching_spans(project_id, records, Backend::Clickhouse)
    else {
        return Ok(HashSet::new());
    };
    let rows: Vec<(String, String, String)> =
        bind_analytics_values(client.query(query.sql()), query.params())
            .fetch_all()
            .await?;
    Ok(rows.into_iter().collect())
}

/// The winning revision of each of these span identities that has one.
pub async fn span_winners(
    client: &Client,
    project_id: &str,
    spans: &[(String, String)],
) -> Result<HashMap<(String, String), SpanWinner>, ClickhouseError> {
    let Some(query) = analytics::span_winners(project_id, spans, Backend::Clickhouse) else {
        return Ok(HashMap::new());
    };
    let rows: Vec<(String, String, String, i64)> =
        bind_analytics_values(client.query(query.sql()), query.params())
            .fetch_all()
            .await?;
    Ok(rows
        .into_iter()
        .map(|(trace_id, span_id, content_digest, ingested_us)| {
            (
                (trace_id, span_id),
                SpanWinner {
                    content_digest,
                    ingested_at: sideseat_core::utils::time::micros_to_datetime(ingested_us),
                },
            )
        })
        .collect())
}

/// Builder for constructing parameterized SQL WHERE clauses.
///
/// Collects conditions and their parameter values, then allows binding
/// all parameters to a ClickHouse query in order.
///
/// # SQL Injection Safety
/// All values that could potentially come from user input are parameterized.
use crate::ClickhouseError;
use rows::{ChSessionRow, ChSpanRow, ChTraceRow};
use sideseat_ports::types::{
    FeedSpansParams, ListSessionsParams, ListSpansParams, ListTracesParams, ProjectId, SessionRow,
    SpanRow, SpanWinner, TraceRow,
};

/// List traces with pagination and filtering
pub async fn list_traces(
    client: &Client,
    params: &ListTracesParams,
) -> Result<(Vec<TraceRow>, u64), ClickhouseError> {
    let page = analytics::list_traces(params, Backend::Clickhouse);
    let total: u64 = bind_analytics_values(client.query(page.count.sql()), page.count.params())
        .fetch_one()
        .await?;
    let rows: Vec<ChTraceRow> =
        bind_analytics_values(client.query(page.rows.sql()), page.rows.params())
            .fetch_all()
            .await?;
    Ok((rows.into_iter().map(TraceRow::from).collect(), total))
}

/// Get spans for a specific trace
pub async fn get_spans_for_trace(
    client: &Client,
    project_id: &str,
    trace_id: &str,
    limit: usize,
) -> Result<Vec<SpanRow>, ClickhouseError> {
    let query = analytics::spans_for_trace(project_id, trace_id, limit, Backend::Clickhouse);
    let rows: Vec<ChSpanRow> = bind_analytics_values(client.query(query.sql()), query.params())
        .fetch_all()
        .await?;

    let spans: Vec<SpanRow> = rows.into_iter().map(SpanRow::from).collect();
    Ok(spans)
}

/// Get a single span
pub async fn get_span(
    client: &Client,
    project_id: &str,
    trace_id: &str,
    span_id: &str,
) -> Result<Option<SpanRow>, ClickhouseError> {
    let query = analytics::span_by_id().render(Backend::Clickhouse);

    let row: Option<ChSpanRow> = client
        .query(query.sql())
        .bind(project_id)
        .bind(trace_id)
        .bind(span_id)
        .fetch_optional()
        .await?;

    Ok(row.map(SpanRow::from))
}

/// List spans with pagination and filtering
pub async fn list_spans(
    client: &Client,
    params: &ListSpansParams,
) -> Result<(Vec<SpanRow>, u64), ClickhouseError> {
    let page = analytics::list_spans(params, Backend::Clickhouse);
    let total: u64 = bind_analytics_values(client.query(page.count.sql()), page.count.params())
        .fetch_one()
        .await?;
    let rows: Vec<ChSpanRow> =
        bind_analytics_values(client.query(page.rows.sql()), page.rows.params())
            .fetch_all()
            .await?;

    Ok((rows.into_iter().map(SpanRow::from).collect(), total))
}

/// Get feed spans (cursor-based pagination for real-time updates)
pub async fn get_feed_spans(
    client: &Client,
    params: &FeedSpansParams,
) -> Result<Vec<SpanRow>, ClickhouseError> {
    let query = analytics::feed_spans(params, Backend::Clickhouse);
    let rows: Vec<ChSpanRow> = bind_analytics_values(client.query(query.sql()), query.params())
        .fetch_all()
        .await?;
    Ok(rows.into_iter().map(SpanRow::from).collect())
}

/// List sessions with pagination and filtering
pub async fn list_sessions(
    client: &Client,
    params: &ListSessionsParams,
) -> Result<(Vec<SessionRow>, u64), ClickhouseError> {
    let page = analytics::list_sessions(params, Backend::Clickhouse);
    let total: u64 = bind_analytics_values(client.query(page.count.sql()), page.count.params())
        .fetch_one()
        .await?;
    let rows: Vec<ChSessionRow> =
        bind_analytics_values(client.query(page.rows.sql()), page.rows.params())
            .fetch_all()
            .await?;
    Ok((rows.into_iter().map(SessionRow::from).collect(), total))
}

/// Get session details
/// session_id is only on root spans; uses session_traces CTE to find all traces,
/// then queries all spans from those traces.
pub async fn get_session(
    client: &Client,
    project_id: &str,
    session_id: &str,
) -> Result<Option<SessionRow>, ClickhouseError> {
    let query = analytics::session_by_id(project_id, session_id, Backend::Clickhouse);
    let row: Option<ChSessionRow> =
        bind_analytics_values(client.query(query.sql()), query.params())
            .fetch_optional()
            .await?;
    Ok(row.map(SessionRow::from))
}

/// Get a single trace by ID
pub async fn get_trace(
    client: &Client,
    project_id: &str,
    trace_id: &str,
) -> Result<Option<TraceRow>, ClickhouseError> {
    let query = analytics::trace_by_id(project_id, trace_id, Backend::Clickhouse);
    let row: Option<ChTraceRow> = bind_analytics_values(client.query(query.sql()), query.params())
        .fetch_optional()
        .await?;
    Ok(row.map(TraceRow::from))
}

/// Get traces for a session
/// session_id is only on root spans; uses session_traces CTE to find all traces,
/// then queries all spans from those traces.
pub async fn get_traces_for_session(
    client: &Client,
    project_id: &str,
    session_id: &str,
) -> Result<Vec<TraceRow>, ClickhouseError> {
    let query = analytics::traces_for_session(project_id, session_id, Backend::Clickhouse);
    let rows: Vec<ChTraceRow> = bind_analytics_values(client.query(query.sql()), query.params())
        .fetch_all()
        .await?;
    Ok(rows.into_iter().map(TraceRow::from).collect())
}

/// Which session each of the given traces belongs to; traces with none are absent.
///
/// Membership is deduplicated at `as_of_us`, or with plain `FINAL` for a current read. The relation uses
/// `argMin` over `(timestamp_start, span_id)` so grouping agrees with the earliest-span session shown by trace
/// and session views. Watermarked reads share the residual documented by
/// `ch_dedup_spans_as_of_watermark`: they are exact only while the pre-watermark revision remains unmerged.
pub async fn get_trace_session_pairs(
    client: &Client,
    project_id: &str,
    trace_ids: &[String],
    as_of_us: Option<i64>,
) -> Result<Vec<(String, String)>, ClickhouseError> {
    let Some(statement) =
        analytics::trace_session_pairs(project_id, trace_ids, as_of_us, Backend::Clickhouse)
    else {
        return Ok(vec![]);
    };

    #[derive(Row, Deserialize)]
    struct PairRow {
        trace_id: String,
        session: String,
    }

    let query = bind_analytics_values(client.query(statement.sql()), statement.params());
    let rows: Vec<PairRow> = query.fetch_all().await?;

    let mut pairs: Vec<(String, String)> =
        rows.into_iter().map(|r| (r.trace_id, r.session)).collect();
    pairs.sort();
    Ok(pairs)
}

/// Get trace IDs for given session IDs
/// The distinct sessions the given traces belong to.
pub async fn get_session_ids_for_traces(
    client: &Client,
    project_id: &str,
    trace_ids: &[String],
    as_of_us: Option<i64>,
) -> Result<Vec<String>, ClickhouseError> {
    let Some(statement) =
        analytics::session_ids_for_traces(project_id, trace_ids, as_of_us, Backend::Clickhouse)
    else {
        return Ok(vec![]);
    };

    #[derive(Row, Deserialize)]
    struct SessionIdRow {
        canonical_session: String,
    }

    let query = bind_analytics_values(client.query(statement.sql()), statement.params());
    let rows: Vec<SessionIdRow> = query.fetch_all().await?;

    let mut session_ids: Vec<String> = rows.into_iter().map(|r| r.canonical_session).collect();
    session_ids.sort();
    session_ids.dedup();
    Ok(session_ids)
}

pub async fn get_trace_ids_for_sessions(
    client: &Client,
    project_id: &str,
    session_ids: &[String],
    as_of_us: Option<i64>,
) -> Result<Vec<String>, ClickhouseError> {
    let Some(statement) =
        analytics::trace_ids_for_sessions(project_id, session_ids, as_of_us, Backend::Clickhouse)
    else {
        return Ok(vec![]);
    };

    #[derive(Row, Deserialize)]
    struct TraceIdRow {
        trace_id: String,
    }

    let query = bind_analytics_values(client.query(statement.sql()), statement.params());
    let rows: Vec<TraceIdRow> = query.fetch_all().await?;

    Ok(rows.into_iter().map(|r| r.trace_id).collect())
}

/// Get span counts (events and links) in bulk
pub async fn get_span_counts_bulk(
    client: &Client,
    project_id: &str,
    spans: &[(String, String)],
) -> Result<
    std::collections::HashMap<(String, String), sideseat_ports::types::SpanCounts>,
    ClickhouseError,
> {
    use sideseat_ports::types::SpanCounts;
    use std::collections::HashMap;

    let Some(statement) = analytics::span_counts_bulk(project_id, spans, Backend::Clickhouse)
    else {
        return Ok(HashMap::new());
    };

    let mut counts: HashMap<(String, String), SpanCounts> = HashMap::with_capacity(spans.len());

    #[derive(Row, Deserialize)]
    struct CountRow {
        trace_id: String,
        span_id: String,
        event_count: u32,
        link_count: u32,
    }

    let query = bind_analytics_values(client.query(statement.sql()), statement.params());
    let rows: Vec<CountRow> = query.fetch_all().await?;

    for row in rows {
        counts.insert(
            (row.trace_id, row.span_id),
            SpanCounts {
                event_count: i64::from(row.event_count),
                link_count: i64::from(row.link_count),
            },
        );
    }

    Ok(counts)
}

/// Delete traces by IDs
///
/// In distributed mode, `table` should be the local table name (e.g., `otel_spans_local`)
/// and `on_cluster` should be the ON CLUSTER clause (e.g., ` ON CLUSTER cluster_name`).
/// Which of these traces have no winning spans left. `FINAL`, for the reason the field query gives.
pub async fn traces_without_spans(
    client: &clickhouse::Client,
    project_id: &str,
    trace_ids: &[String],
) -> Result<Vec<String>, ClickhouseError> {
    let Some(statement) =
        analytics::surviving_trace_ids(project_id, trace_ids, Backend::Clickhouse)
    else {
        return Ok(Vec::new());
    };
    let query = bind_analytics_values(client.query(statement.sql()), statement.params());
    let alive: Vec<String> = query.fetch_all().await?;
    Ok(trace_ids
        .iter()
        .filter(|t| !alive.contains(t))
        .cloned()
        .collect())
}

/// The text of every field that can hold a `#!B64!#` reference, for the surviving winning spans of these
/// traces.
///
/// `FINAL`, so an expired revision's text does not keep an association alive on behalf of a span that is no
/// longer current - the DuckDB side reads `DEDUP_SPANS` for the same reason.
pub async fn file_reference_fields_for_traces(
    client: &clickhouse::Client,
    project_id: &str,
    trace_ids: &[String],
) -> Result<Vec<String>, ClickhouseError> {
    let Some(statement) =
        analytics::file_reference_fields(project_id, trace_ids, Backend::Clickhouse)
    else {
        return Ok(Vec::new());
    };
    let query = bind_analytics_values(client.query(statement.sql()), statement.params());
    let rows: Vec<(String, String, String)> = query.fetch_all().await?;
    Ok(rows
        .into_iter()
        .flat_map(|(messages, tool_definitions, metadata)| [messages, tool_definitions, metadata])
        .filter(|text| !text.is_empty())
        .collect())
}

pub async fn delete_traces(
    client: &Client,
    table: &str,
    logs_table: &str,
    on_cluster: &str,
    project_id: &str,
    trace_ids: &[String],
) -> Result<u64, ClickhouseError> {
    let Some(statement) = dml::delete_traces(
        dml::MutationTarget::clickhouse(table, on_cluster),
        project_id,
        trace_ids,
    ) else {
        return Ok(0);
    };

    // The records the rows name are enqueued before the rows go; see `dml::raw::enqueue_raw_for_traces`.
    let enqueue = dml::raw::enqueue_raw_for_traces(Backend::Clickhouse, project_id, trace_ids)
        .expect("non-empty trace set produces a raw enqueue");
    bind_analytics_values(client.query(enqueue.sql()), enqueue.params())
        .execute()
        .await?;
    let query = bind_analytics_values(client.query(statement.sql()), statement.params());
    query.execute().await?;
    let logs = dml::delete_logs_for_traces(
        dml::MutationTarget::clickhouse(logs_table, on_cluster),
        project_id,
        trace_ids,
    )
    .expect("non-empty trace set produces a log delete");
    bind_analytics_values(client.query(logs.sql()), logs.params())
        .execute()
        .await?;

    // Return count - mutations are async in ClickHouse so we estimate
    Ok(trace_ids.len() as u64)
}

/// Delete spans by (trace_id, span_id) pairs
///
/// In distributed mode, `table` should be the local table name and
/// `on_cluster` should be the ON CLUSTER clause.
pub async fn delete_spans(
    client: &Client,
    table: &str,
    logs_table: &str,
    on_cluster: &str,
    project_id: &str,
    spans: &[(String, String)],
) -> Result<u64, ClickhouseError> {
    let Some(statement) = dml::delete_spans(
        dml::MutationTarget::clickhouse(table, on_cluster),
        project_id,
        spans,
    ) else {
        return Ok(0);
    };

    let enqueue = dml::raw::enqueue_raw_for_spans(Backend::Clickhouse, project_id, spans)
        .expect("non-empty span set produces a raw enqueue");
    bind_analytics_values(client.query(enqueue.sql()), enqueue.params())
        .execute()
        .await?;
    let query = bind_analytics_values(client.query(statement.sql()), statement.params());
    query.execute().await?;
    let logs = dml::delete_logs_for_spans(
        dml::MutationTarget::clickhouse(logs_table, on_cluster),
        project_id,
        spans,
    )
    .expect("non-empty span set produces a log delete");
    bind_analytics_values(client.query(logs.sql()), logs.params())
        .execute()
        .await?;

    Ok(spans.len() as u64)
}

/// Delete sessions (all spans in the sessions)
///
/// In distributed mode, `table` should be the local table name and
/// `on_cluster` should be the ON CLUSTER clause.
pub async fn delete_sessions(
    client: &Client,
    table: &str,
    logs_table: &str,
    on_cluster: &str,
    project_id: &str,
    session_ids: &[String],
) -> Result<Vec<String>, ClickhouseError> {
    if session_ids.is_empty() {
        return Ok(vec![]);
    }

    // Resolve sessions to complete trace sets because session ids commonly appear only on root spans.
    // No watermark: a deletion acts on what is stored *now*, not as of some past instant.
    let trace_ids = get_trace_ids_for_sessions(client, project_id, session_ids, None).await?;
    if trace_ids.is_empty() {
        return Ok(vec![]);
    }
    let statement = dml::delete_session_traces(
        dml::MutationTarget::clickhouse(table, on_cluster),
        project_id,
        &trace_ids,
    )
    .expect("non-empty trace set produces a delete");
    let enqueue = dml::raw::enqueue_raw_for_traces(Backend::Clickhouse, project_id, &trace_ids)
        .expect("non-empty trace set produces a raw enqueue");
    bind_analytics_values(client.query(enqueue.sql()), enqueue.params())
        .execute()
        .await?;
    bind_analytics_values(client.query(statement.sql()), statement.params())
        .execute()
        .await?;
    let logs = dml::delete_logs_for_traces(
        dml::MutationTarget::clickhouse(logs_table, on_cluster),
        project_id,
        &trace_ids,
    )
    .expect("non-empty trace set produces a log delete");
    bind_analytics_values(client.query(logs.sql()), logs.params())
        .execute()
        .await?;
    Ok(trace_ids)
}

/// Delete all data for a project
///
/// In distributed mode, `spans_table` and `metrics_table` should be local table names
/// and `on_cluster` should be the ON CLUSTER clause.
#[allow(clippy::too_many_arguments)]
pub async fn delete_project_data(
    client: &Client,
    spans_table: &str,
    metrics_table: &str,
    logs_table: &str,
    raw_table: &str,
    raw_pending_table: &str,
    raw_traces_table: &str,
    on_cluster: &str,
    project_id: &str,
) -> Result<u64, ClickhouseError> {
    let plan = dml::delete_project_data(
        dml::MutationTarget::clickhouse(spans_table, on_cluster),
        dml::MutationTarget::clickhouse(metrics_table, on_cluster),
        dml::MutationTarget::clickhouse(logs_table, on_cluster),
        dml::MutationTarget::clickhouse(raw_table, on_cluster),
        dml::MutationTarget::clickhouse(raw_pending_table, on_cluster),
        dml::MutationTarget::clickhouse(raw_traces_table, on_cluster),
        project_id,
    );
    let count_statement = plan
        .count_spans
        .as_ref()
        .expect("ClickHouse project deletion has a count statement");
    let count: u64 = bind_analytics_values(
        client.query(count_statement.sql()),
        count_statement.params(),
    )
    .fetch_one()
    .await?;

    bind_analytics_values(
        client.query(plan.delete_spans.sql()),
        plan.delete_spans.params(),
    )
    .execute()
    .await?;

    // Delete metrics when their table exists; query failures remain deletion failures rather than being
    // mistaken for an absent optional table.
    let table_check = plan
        .metrics_table_exists
        .as_ref()
        .expect("ClickHouse project deletion checks the metrics table");
    let table_exists: u64 =
        bind_analytics_values(client.query(table_check.sql()), table_check.params())
            .fetch_one()
            .await?;
    if table_exists > 0 {
        bind_analytics_values(
            client.query(plan.delete_metrics.sql()),
            plan.delete_metrics.params(),
        )
        .execute()
        .await?;
    }

    for statement in [
        &plan.delete_logs,
        &plan.delete_raw,
        &plan.delete_raw_pending,
        &plan.delete_raw_traces,
    ] {
        bind_analytics_values(client.query(statement.sql()), statement.params())
            .execute()
            .await?;
    }

    Ok(count)
}

/// Count every row a project still owns, spans and metrics together.
///
/// `FINAL` on both, because a `ReplacingMergeTree` may still hold superseded parts - and because this is
/// read to decide whether a deleted project's data is really gone, an approximate answer is the wrong
/// kind of answer. The metrics table is asked only if it exists, for the same reason its delete is.
pub async fn count_project_rows(
    client: &Client,
    metrics_table: &str,
    project_id: &str,
) -> Result<u64, ClickhouseError> {
    let plan = analytics::project_row_count(project_id, Backend::Clickhouse, Some(metrics_table));
    let spans: u64 = bind_analytics_values(client.query(plan.spans.sql()), plan.spans.params())
        .fetch_one()
        .await?;

    let table_check = plan
        .metrics_table_exists
        .as_ref()
        .expect("ClickHouse count plan checks metric table");
    let table_exists: u64 =
        bind_analytics_values(client.query(table_check.sql()), table_check.params())
            .fetch_one()
            .await?;
    let metrics: u64 = if table_exists > 0 {
        bind_analytics_values(client.query(plan.metrics.sql()), plan.metrics.params())
            .fetch_one()
            .await?
    } else {
        0
    };
    let logs: u64 = bind_analytics_values(client.query(plan.logs.sql()), plan.logs.params())
        .fetch_one()
        .await?;
    Ok(spans + metrics + logs)
}

#[allow(clippy::too_many_arguments)]
pub async fn patch_project_hold(
    client: &Client,
    spans_table: &str,
    metrics_table: &str,
    logs_table: &str,
    raw_table: &str,
    raw_traces_table: &str,
    on_cluster: &str,
    project_id: &str,
    hold_until: chrono::DateTime<Utc>,
) -> Result<(), ClickhouseError> {
    let statements = dml::patch_project_hold(
        dml::MutationTarget::clickhouse(spans_table, on_cluster),
        dml::MutationTarget::clickhouse(metrics_table, on_cluster),
        dml::MutationTarget::clickhouse(logs_table, on_cluster),
        dml::MutationTarget::clickhouse(raw_table, on_cluster),
        dml::MutationTarget::clickhouse(raw_traces_table, on_cluster),
        project_id,
        hold_until,
    );
    for statement in &statements {
        bind_analytics_values(client.query(statement.sql()), statement.params())
            .execute()
            .await?;
    }
    Ok(())
}

pub async fn project_logical_bytes(
    client: &Client,
    project_id: &str,
    held_at: Option<chrono::DateTime<Utc>>,
) -> Result<u64, ClickhouseError> {
    let plan = analytics::project_logical_bytes(project_id, Backend::Clickhouse, held_at);
    let spans: u64 = bind_analytics_values(client.query(plan.spans.sql()), plan.spans.params())
        .fetch_one()
        .await?;
    let metrics: u64 =
        bind_analytics_values(client.query(plan.metrics.sql()), plan.metrics.params())
            .fetch_one()
            .await?;
    let logs: u64 = bind_analytics_values(client.query(plan.logs.sql()), plan.logs.params())
        .fetch_one()
        .await?;
    Ok(spans.saturating_add(metrics).saturating_add(logs))
}

pub async fn oldest_reclaimable_spans(
    client: &Client,
    project_id: &str,
    target_bytes: u64,
    now: chrono::DateTime<Utc>,
    limit: usize,
) -> Result<Vec<sideseat_ports::types::PressureSpanCandidate>, ClickhouseError> {
    let statement = analytics::oldest_reclaimable_spans(
        project_id,
        Backend::Clickhouse,
        target_bytes,
        now,
        limit,
    );
    let rows: Vec<(String, String, u64)> =
        bind_analytics_values(client.query(statement.sql()), statement.params())
            .fetch_all()
            .await?;
    Ok(rows
        .into_iter()
        .map(
            |(trace_id, span_id, logical_bytes)| sideseat_ports::types::PressureSpanCandidate {
                trace_id,
                span_id,
                logical_bytes,
            },
        )
        .collect())
}

/// The newest committed ingestion time for a project, in microseconds.
pub async fn max_ingested_at_us(
    client: &Client,
    project_id: &str,
) -> Result<Option<i64>, ClickhouseError> {
    let statement = analytics::max_ingested_at_us(project_id, Backend::Clickhouse);
    let value: Option<i64> =
        bind_analytics_values(client.query(statement.sql()), statement.params())
            .fetch_optional()
            .await?;
    // ClickHouse answers max() over an empty set with zero rather than null.
    Ok(value.filter(|value| *value > 0))
}

pub async fn analytics_project_ids(
    client: &Client,
    limit: usize,
) -> Result<Vec<ProjectId>, ClickhouseError> {
    #[derive(Row, Deserialize)]
    struct ProjectRow {
        project_id: String,
    }

    let statement = analytics::analytics_project_ids(Backend::Clickhouse, limit);
    let rows: Vec<ProjectRow> =
        bind_analytics_values(client.query(statement.sql()), statement.params())
            .fetch_all()
            .await?;
    Ok(rows
        .into_iter()
        .map(|row| ProjectId::from(row.project_id))
        .collect())
}

pub async fn count_spans_by_project(
    client: &Client,
    project_ids: &[String],
) -> Result<std::collections::HashMap<String, u64>, ClickhouseError> {
    use std::collections::HashMap;

    let Some(statement) = analytics::span_counts_by_project(project_ids, Backend::Clickhouse)
    else {
        return Ok(HashMap::new());
    };

    #[derive(Row, Deserialize)]
    struct CountRow {
        project_id: String,
        cnt: u64,
    }

    let query = bind_analytics_values(client.query(statement.sql()), statement.params());
    let rows: Vec<CountRow> = query.fetch_all().await?;

    let mut result = HashMap::new();
    for row in rows {
        result.insert(row.project_id, row.cnt);
    }

    Ok(result)
}

/// ClickHouse row for filter options
#[derive(Row, Deserialize)]
struct ChFilterOptionRow {
    value: Option<String>,
    count: u64,
}

async fn fetch_filter_option_rows(
    client: &Client,
    query: &analytics::ParameterizedQuery,
) -> Result<Vec<ChFilterOptionRow>, ClickhouseError> {
    Ok(
        bind_analytics_values(client.query(query.sql()), query.params())
            .fetch_all()
            .await?,
    )
}

fn plain_filter_options(
    rows: Vec<ChFilterOptionRow>,
) -> Vec<sideseat_ports::traits::FilterOptionRow> {
    rows.into_iter()
        .filter_map(|row| {
            row.value
                .map(|value| sideseat_ports::traits::FilterOptionRow {
                    value,
                    count: row.count,
                })
        })
        .collect()
}

/// Get trace filter options
pub async fn get_trace_filter_options(
    client: &Client,
    project_id: &str,
    columns: &[String],
    from_timestamp: Option<DateTime<Utc>>,
    to_timestamp: Option<DateTime<Utc>>,
) -> Result<
    std::collections::HashMap<String, Vec<sideseat_ports::traits::FilterOptionRow>>,
    ClickhouseError,
> {
    let mut results = std::collections::HashMap::new();
    for option in analytics::trace_filter_options(
        project_id,
        columns,
        from_timestamp.as_ref(),
        to_timestamp.as_ref(),
        Backend::Clickhouse,
    ) {
        let rows = fetch_filter_option_rows(client, &option.query).await?;
        results.insert(option.column, plain_filter_options(rows));
    }
    Ok(results)
}

/// Get trace tags options
pub async fn get_trace_tags_options(
    client: &Client,
    project_id: &str,
    from_timestamp: Option<DateTime<Utc>>,
    to_timestamp: Option<DateTime<Utc>>,
) -> Result<Vec<sideseat_ports::traits::FilterOptionRow>, ClickhouseError> {
    let query = analytics::trace_tag_options(
        project_id,
        from_timestamp.as_ref(),
        to_timestamp.as_ref(),
        Backend::Clickhouse,
    );
    let rows = fetch_filter_option_rows(client, &query).await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            row.value
                .map(|raw| sideseat_ports::traits::FilterOptionRow {
                    value: serde_json::from_str::<String>(&raw)
                        .unwrap_or_else(|_| raw.trim_matches('"').to_string()),
                    count: row.count,
                })
        })
        .collect())
}

/// Get span filter options
pub async fn get_span_filter_options(
    client: &Client,
    project_id: &str,
    columns: &[String],
    from_timestamp: Option<DateTime<Utc>>,
    to_timestamp: Option<DateTime<Utc>>,
    observations_only: bool,
) -> Result<
    std::collections::HashMap<String, Vec<sideseat_ports::traits::FilterOptionRow>>,
    ClickhouseError,
> {
    let mut results = std::collections::HashMap::new();
    for option in analytics::span_filter_options(
        project_id,
        columns,
        from_timestamp.as_ref(),
        to_timestamp.as_ref(),
        observations_only,
        Backend::Clickhouse,
    ) {
        let rows = fetch_filter_option_rows(client, &option.query).await?;
        results.insert(option.column, plain_filter_options(rows));
    }
    Ok(results)
}

/// Get session filter options
pub async fn get_session_filter_options(
    client: &Client,
    project_id: &str,
    columns: &[String],
    from_timestamp: Option<DateTime<Utc>>,
    to_timestamp: Option<DateTime<Utc>>,
) -> Result<
    std::collections::HashMap<String, Vec<sideseat_ports::traits::FilterOptionRow>>,
    ClickhouseError,
> {
    let mut results = std::collections::HashMap::new();
    for option in analytics::session_filter_options(
        project_id,
        columns,
        from_timestamp.as_ref(),
        to_timestamp.as_ref(),
        Backend::Clickhouse,
    ) {
        let rows = fetch_filter_option_rows(client, &option.query).await?;
        results.insert(option.column, plain_filter_options(rows));
    }
    Ok(results)
}

/// Repository contract tests.
#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;
