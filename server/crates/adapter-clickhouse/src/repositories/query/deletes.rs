//! Deleting traces, spans and sessions: each chunk of identities a mutation of its own, its raw records enqueued
//! before its span and log rows go.

use clickhouse::Client;
use sideseat_query_sql::{Backend, dml};

use super::{bind_analytics_values, get_trace_ids_for_sessions};
use crate::ClickhouseError;

/// Identities per deleting mutation. The driver writes every bound value into the statement, and
/// `max_query_size` (256 KiB by default) refuses one past it: 1,024 span pairs are under 145 KB even with 64-character
/// ids, and 1,024 trace ids under 70 KB. Each
/// chunk is a mutation of its own, so a larger set costs more mutations, not a larger statement; a set stops
/// part-way only where every caller already retries the whole - a mutation that finds nothing deletes nothing, and
/// a record enqueued twice is reconciled once.
const MUTATION_CHUNK: usize = 1_024;

pub async fn delete_traces(
    client: &Client,
    table: &str,
    logs_table: &str,
    on_cluster: &str,
    project_id: &str,
    trace_ids: &[String],
) -> Result<u64, ClickhouseError> {
    for chunk in sideseat_query_sql::keyed::bounded_chunks(trace_ids, MUTATION_CHUNK) {
        let statement = dml::delete_traces(
            dml::MutationTarget::clickhouse(table, on_cluster),
            project_id,
            chunk,
        )
        .expect("non-empty trace set produces a delete");
        delete_trace_rows(
            client, logs_table, on_cluster, project_id, chunk, &statement,
        )
        .await?;
    }
    // Return count - mutations are async in ClickHouse so we estimate
    Ok(trace_ids.len() as u64)
}

/// One chunk of traces' rows deleted: the records the rows name enqueued before the rows go
/// (`dml::raw::enqueue_raw_for_traces`), then the span rows by `spans`, then the log rows.
async fn delete_trace_rows(
    client: &Client,
    logs_table: &str,
    on_cluster: &str,
    project_id: &str,
    trace_ids: &[String],
    spans: &dml::DmlStatement,
) -> Result<(), ClickhouseError> {
    let enqueue = dml::raw::enqueue_raw_for_traces(Backend::Clickhouse, project_id, trace_ids)
        .expect("non-empty trace set produces a raw enqueue");
    bind_analytics_values(client.query(enqueue.sql()), enqueue.params())
        .execute()
        .await?;
    bind_analytics_values(client.query(spans.sql()), spans.params())
        .execute()
        .await?;
    let logs = dml::delete_logs_for_traces(
        dml::MutationTarget::clickhouse(logs_table, on_cluster),
        project_id,
        trace_ids,
    )
    .expect("non-empty trace set produces a log delete");
    bind_analytics_values(client.query(logs.sql()), logs.params())
        .execute()
        .await?;
    Ok(())
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
    // A chunk at a time (`MUTATION_CHUNK`): storage pressure evicts up to 100,000 spans in one call. The records of
    // the spans' distinct traces are enqueued once, every chunk of them before any row goes - a trace split across
    // span chunks would otherwise be enqueued once per chunk - then the span and log rows a chunk at a time.
    let traces: Vec<String> = sideseat_query_sql::keyed::distinct_keys(
        spans.iter().map(|(trace_id, _)| trace_id.as_str()),
    )
    .into_iter()
    .map(str::to_string)
    .collect();
    for chunk in sideseat_query_sql::keyed::bounded_chunks(&traces, MUTATION_CHUNK) {
        let enqueue = dml::raw::enqueue_raw_for_traces(Backend::Clickhouse, project_id, chunk)
            .expect("non-empty trace set produces a raw enqueue");
        bind_analytics_values(client.query(enqueue.sql()), enqueue.params())
            .execute()
            .await?;
    }
    for chunk in sideseat_query_sql::keyed::bounded_chunks(spans, MUTATION_CHUNK) {
        let statement = dml::delete_spans(
            dml::MutationTarget::clickhouse(table, on_cluster),
            project_id,
            chunk,
        )
        .expect("non-empty span set produces a delete");
        bind_analytics_values(client.query(statement.sql()), statement.params())
            .execute()
            .await?;
        let logs = dml::delete_logs_for_spans(
            dml::MutationTarget::clickhouse(logs_table, on_cluster),
            project_id,
            chunk,
        )
        .expect("non-empty span set produces a log delete");
        bind_analytics_values(client.query(logs.sql()), logs.params())
            .execute()
            .await?;
    }
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
    // The traces are resolved once, above, and deleted a chunk at a time: a session holds as many as it held.
    for chunk in sideseat_query_sql::keyed::bounded_chunks(&trace_ids, MUTATION_CHUNK) {
        let statement = dml::delete_session_traces(
            dml::MutationTarget::clickhouse(table, on_cluster),
            project_id,
            chunk,
        )
        .expect("non-empty trace set produces a delete");
        delete_trace_rows(
            client, logs_table, on_cluster, project_id, chunk, &statement,
        )
        .await?;
    }
    Ok(trace_ids)
}
