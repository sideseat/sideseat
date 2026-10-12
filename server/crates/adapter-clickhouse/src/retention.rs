//! ClickHouse execution for the shared retention mutation.

use chrono::{DateTime, Utc};
use clickhouse::Client;
use sideseat_query_sql::analytics::QueryValue;
use sideseat_query_sql::dml::{self, MutationTarget};

use crate::ClickhouseError;

/// Traces recorded as owing a cleanup at once, while the expiring ones are streamed.
const EXPIRING_TRACES_PER_RECORD: usize = 1024;

/// Record a retention cleanup for every trace the age delete is about to touch, before it runs - the intent the
/// retention cleanup loop later finishes (files, favourites, raw-record ownership), and the only thing that knows
/// the cleanup is owed once the rows are gone. Read across every replica (`spans`), so no trace whose rows an
/// acknowledged insert put on one replica is deleted without its intent; streamed, so a first pass over a large
/// history holds one chunk of trace ids at a time. Returns the traces recorded.
pub async fn record_expiring_traces(
    client: &Client,
    spans: &str,
    project_id: &sideseat_ports::types::ProjectId,
    cutoff: DateTime<Utc>,
    now: DateTime<Utc>,
    record: &(dyn sideseat_ports::traits::TransactionalRepository + Send + Sync),
) -> Result<usize, String> {
    let statement = dml::retention::retention_expiring_traces_clickhouse(
        spans,
        project_id.as_str(),
        cutoff,
        now,
    );
    let mut query = client.query(statement.sql());
    for value in statement.params() {
        query = match value {
            QueryValue::String(value) => query.bind(value),
            QueryValue::Int64(value) => query.bind(value),
            QueryValue::Float64(value) => query.bind(value),
        };
    }
    let mut cursor = query.fetch::<String>().map_err(|error| error.to_string())?;
    let mut chunk = Vec::with_capacity(EXPIRING_TRACES_PER_RECORD);
    let mut recorded = 0usize;
    loop {
        let next = cursor.next().await.map_err(|error| error.to_string())?;
        let done = next.is_none();
        if let Some(trace_id) = next {
            chunk.push(trace_id);
        }
        if chunk.len() >= EXPIRING_TRACES_PER_RECORD || (done && !chunk.is_empty()) {
            record
                .record_retention_cleanup(project_id, &chunk)
                .await
                .map_err(|error| error.to_string())?;
            recorded += chunk.len();
            chunk.clear();
        }
        if done {
            return Ok(recorded);
        }
    }
}

/// Delete spans older than the configured retention boundary.
#[allow(clippy::too_many_arguments)]
pub async fn run_retention(
    client: &Client,
    spans_table: &str,
    metrics_table: &str,
    logs_table: &str,
    on_cluster: &str,
    project_id: &str,
    cutoff: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<(), ClickhouseError> {
    let statement = dml::retention::retention_delete_expired_clickhouse(
        MutationTarget::clickhouse(spans_table, on_cluster),
        project_id,
        cutoff,
        now,
    );
    let metrics = dml::retention::retention_delete_expired_metrics_clickhouse(
        MutationTarget::clickhouse(metrics_table, on_cluster),
        project_id,
        cutoff,
        now,
    );
    let logs = dml::retention::retention_delete_expired_logs_clickhouse(
        MutationTarget::clickhouse(logs_table, on_cluster),
        project_id,
        cutoff,
        now,
    );
    // The records the expiring rows name, enqueued first with the same predicate.
    let enqueue = dml::raw::enqueue_raw_for_expired_clickhouse(project_id, cutoff, now);
    for statement in [&enqueue, &statement, &metrics, &logs] {
        let mut query = client.query(statement.sql());
        for value in statement.params() {
            query = match value {
                QueryValue::String(value) => query.bind(value),
                QueryValue::Int64(value) => query.bind(value),
                QueryValue::Float64(value) => query.bind(value),
            };
        }
        query.execute().await?;
    }
    Ok(())
}
