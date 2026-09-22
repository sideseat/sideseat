//! ClickHouse execution for the shared retention mutation.

use chrono::{DateTime, Utc};
use clickhouse::Client;
use sideseat_query_sql::analytics::QueryValue;
use sideseat_query_sql::dml::{self, MutationTarget};

use crate::ClickhouseError;

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
    let statement = dml::retention_delete_expired_clickhouse(
        MutationTarget::clickhouse(spans_table, on_cluster),
        project_id,
        cutoff,
        now,
    );
    let metrics = dml::retention_delete_expired_metrics_clickhouse(
        MutationTarget::clickhouse(metrics_table, on_cluster),
        project_id,
        cutoff,
        now,
    );
    let logs = dml::retention_delete_expired_logs_clickhouse(
        MutationTarget::clickhouse(logs_table, on_cluster),
        project_id,
        cutoff,
        now,
    );
    for statement in [&statement, &metrics, &logs] {
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
